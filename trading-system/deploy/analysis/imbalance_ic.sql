-- imbalance_ic.sql — Spearman rank IC: top-of-book imbalance -> forward mid return
--
-- Pre-registration: docs/superpowers/specs/2026-06-16-orderbook-imbalance-preregistration.md
-- Hypothesis: I = bid_size/(bid_size+ask_size) predicts forward mid-price return,
--             sign +, at horizons H in {10s, 30s, 60s}, per exchange x symbol (NO pooling).
--
-- Discipline locked by the pre-reg (do not change after seeing numbers):
--   * forward return on MID price (best_bid+best_ask)/2, never close (bid-ask bounce trap)
--   * NON-OVERLAPPING samples: one base point every H seconds (overlap inflates significance)
--   * forward row = first row in [t+H, t+H+2s); NO interpolation (don't invent data)
--   * per (exchange, symbol, horizon); 18 tests total -> Bonferroni alpha = 0.05/18 = 0.00278
--   * sample-count floor = 2000 non-overlapping points before any judgment
--
-- Spearman IC = Pearson corr() on within-group ranks of (imbalance, forward_return).
-- Approx two-sided significance via Fisher z: z = atanh(r) * sqrt(n-3); |z|>3.20 ~ Bonferroni.
--
-- Run on the capture host:
--   docker exec trading-capture-postgres psql -U trading -d trading_system -f - < imbalance_ic.sql
--   (or paste; or psql ... -f imbalance_ic.sql if the file is on the host)

\set ON_ERROR_STOP on

WITH params AS (
    -- horizons (seconds) + the per-test floor; edit horizons here only, never per-result
    SELECT 2000::bigint AS sample_floor,
           2.0::numeric AS tol_secs        -- forward-match tolerance (1s sampling -> 2s)
),
horizons(h_secs) AS (VALUES (10::int), (30), (60)),

-- one clean row per (exchange, symbol, second): the capture already downsamples to
-- ~1/sec, but guard against any dup second by taking the earliest event in each second.
ticks AS (
    SELECT exchange,
           symbol,
           date_trunc('second', event_time) AS ts,
           (best_bid + best_ask) / 2.0      AS mid,
           bid_size / (bid_size + ask_size) AS imbalance
    FROM (
        SELECT exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size,
               row_number() OVER (
                   PARTITION BY exchange, symbol, date_trunc('second', event_time)
                   ORDER BY event_time
               ) AS rn
        FROM order_books
        WHERE bid_size + ask_size > 0          -- drop empty/0-sum books (pre-reg sec 2)
    ) d
    WHERE rn = 1
),

-- non-overlapping base samples: keep a tick only when its epoch-second is a multiple
-- of the horizon (so 10s -> every 10th second, 60s -> every 60th). Per pre-reg sec 4.2,
-- judgment uses these non-overlapping points only.
base AS (
    SELECT t.exchange, t.symbol, h.h_secs,
           t.ts, t.mid, t.imbalance,
           extract(epoch FROM t.ts)::bigint AS epoch_s
    FROM ticks t
    CROSS JOIN horizons h
    WHERE (extract(epoch FROM t.ts)::bigint % h.h_secs) = 0
),

-- forward match: first tick in [ts + H, ts + H + tol). No interpolation; unmatched dropped.
samples AS (
    SELECT b.exchange, b.symbol, b.h_secs,
           b.imbalance,
           f.mid / b.mid - 1.0 AS fwd_return
    FROM base b
    CROSS JOIN params p
    JOIN LATERAL (
        SELECT t2.mid
        FROM ticks t2
        WHERE t2.exchange = b.exchange
          AND t2.symbol   = b.symbol
          AND t2.ts >= b.ts + make_interval(secs => b.h_secs)
          AND t2.ts <  b.ts + make_interval(secs => b.h_secs) + make_interval(secs => p.tol_secs)
        ORDER BY t2.ts
        LIMIT 1
    ) f ON TRUE
    WHERE b.mid > 0
),

-- within-group ranks for Spearman
ranked AS (
    SELECT exchange, symbol, h_secs,
           rank() OVER (PARTITION BY exchange, symbol, h_secs ORDER BY imbalance)  AS rx,
           rank() OVER (PARTITION BY exchange, symbol, h_secs ORDER BY fwd_return) AS ry
    FROM samples
),

ic AS (
    SELECT exchange, symbol, h_secs,
           count(*)            AS n,
           corr(rx::float8, ry::float8) AS spearman_ic
    FROM ranked
    GROUP BY exchange, symbol, h_secs
)

SELECT
    i.exchange,
    i.symbol,
    i.h_secs                                            AS horizon_s,
    i.n                                                 AS samples,
    round(i.spearman_ic::numeric, 5)                    AS spearman_ic,
    -- Fisher-z two-sided test statistic (|z| > 3.20 ~ Bonferroni alpha 0.00278)
    CASE WHEN i.n > 3 AND i.spearman_ic IS NOT NULL AND abs(i.spearman_ic) < 1
         THEN round((atanh(i.spearman_ic) * sqrt(i.n - 3))::numeric, 2)
    END                                                 AS fisher_z,
    CASE
        WHEN i.n < (SELECT sample_floor FROM params)            THEN 'INCONCLUSIVE (n<floor)'
        WHEN i.spearman_ic IS NULL                              THEN 'INCONCLUSIVE (no var)'
        WHEN abs(atanh(i.spearman_ic) * sqrt(i.n - 3)) <= 3.20  THEN 'no-signal (not sig)'
        WHEN i.spearman_ic < 0                                  THEN 'SIGN VIOLATION (neg IC)'
        ELSE 'signal? (+ & Bonferroni-sig)'                     -- still needs quantile monotonicity
    END                                                 AS verdict
FROM ic i
ORDER BY i.exchange, i.symbol, i.h_secs;
