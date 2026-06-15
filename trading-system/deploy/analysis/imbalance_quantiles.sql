-- imbalance_quantiles.sql — imbalance quintile -> mean forward mid return
--
-- Pre-registration: docs/superpowers/specs/2026-06-16-orderbook-imbalance-preregistration.md
-- Companion to imbalance_ic.sql. Where IC measures monotone rank correlation, this shows the
-- SHAPE: bucket imbalance into quintiles within each (exchange, symbol, horizon) and report
-- mean forward return per bucket. Signal -> Q1..Q5 monotone increasing (hypothesis sign +)
-- and Q5-Q1 spread positive. A positive IC with a non-monotone shape is NOT a clean signal.
--
-- Same locked discipline as imbalance_ic.sql: mid-price returns, non-overlapping samples,
-- per (exchange, symbol, horizon), no pooling, no interpolation. Floor = 2000 samples/feed.
--
-- Run on the capture host:
--   docker exec trading-capture-postgres psql -U trading -d trading_system -f - < imbalance_quantiles.sql

\set ON_ERROR_STOP on

WITH params AS (
    SELECT 2000::bigint AS sample_floor, 2.0::numeric AS tol_secs
),
horizons(h_secs) AS (VALUES (10::int), (30), (60)),

ticks AS (
    SELECT exchange, symbol, date_trunc('second', event_time) AS ts,
           (best_bid + best_ask) / 2.0      AS mid,
           bid_size / (bid_size + ask_size) AS imbalance
    FROM (
        SELECT exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size,
               row_number() OVER (PARTITION BY exchange, symbol, date_trunc('second', event_time)
                                  ORDER BY event_time) AS rn
        FROM order_books
        WHERE bid_size + ask_size > 0
    ) d
    WHERE rn = 1
),

base AS (
    SELECT t.exchange, t.symbol, h.h_secs, t.ts, t.mid, t.imbalance
    FROM ticks t
    CROSS JOIN horizons h
    WHERE (extract(epoch FROM t.ts)::bigint % h.h_secs) = 0
),

samples AS (
    SELECT b.exchange, b.symbol, b.h_secs,
           b.imbalance,
           f.mid / b.mid - 1.0 AS fwd_return
    FROM base b
    CROSS JOIN params p
    JOIN LATERAL (
        SELECT t2.mid FROM ticks t2
        WHERE t2.exchange = b.exchange AND t2.symbol = b.symbol
          AND t2.ts >= b.ts + make_interval(secs => b.h_secs)
          AND t2.ts <  b.ts + make_interval(secs => b.h_secs) + make_interval(secs => p.tol_secs)
        ORDER BY t2.ts LIMIT 1
    ) f ON TRUE
    WHERE b.mid > 0
),

-- quintile within each (exchange, symbol, horizon) by imbalance
bucketed AS (
    SELECT exchange, symbol, h_secs, imbalance, fwd_return,
           ntile(5) OVER (PARTITION BY exchange, symbol, h_secs ORDER BY imbalance) AS q
    FROM samples
),

per_bucket AS (
    SELECT exchange, symbol, h_secs, q,
           count(*)                         AS n,
           round(avg(imbalance)::numeric, 4)         AS avg_imbalance,
           round((avg(fwd_return) * 10000)::numeric, 3) AS mean_ret_bps  -- basis points
    FROM bucketed
    GROUP BY exchange, symbol, h_secs, q
),

-- pivot quintiles to columns + Q5-Q1 spread + monotonicity check
shaped AS (
    SELECT exchange, symbol, h_secs,
           sum(n)                                              AS total_n,
           max(CASE WHEN q=1 THEN mean_ret_bps END) AS q1_bps,
           max(CASE WHEN q=2 THEN mean_ret_bps END) AS q2_bps,
           max(CASE WHEN q=3 THEN mean_ret_bps END) AS q3_bps,
           max(CASE WHEN q=4 THEN mean_ret_bps END) AS q4_bps,
           max(CASE WHEN q=5 THEN mean_ret_bps END) AS q5_bps
    FROM per_bucket
    GROUP BY exchange, symbol, h_secs
)
SELECT
    s.exchange, s.symbol, s.h_secs AS horizon_s, s.total_n AS samples,
    s.q1_bps, s.q2_bps, s.q3_bps, s.q4_bps, s.q5_bps,
    round((s.q5_bps - s.q1_bps), 3) AS spread_q5_q1_bps,
    -- strictly monotone increasing Q1<Q2<Q3<Q4<Q5 == hypothesis shape
    (s.q1_bps < s.q2_bps AND s.q2_bps < s.q3_bps
        AND s.q3_bps < s.q4_bps AND s.q4_bps < s.q5_bps) AS monotone_up,
    CASE
        WHEN s.total_n < (SELECT sample_floor FROM params) THEN 'INCONCLUSIVE (n<floor)'
        WHEN s.q5_bps > s.q1_bps
             AND (s.q1_bps < s.q2_bps AND s.q2_bps < s.q3_bps
                  AND s.q3_bps < s.q4_bps AND s.q4_bps < s.q5_bps) THEN 'shape OK (monotone +)'
        WHEN s.q5_bps > s.q1_bps                                   THEN 'weak (+ spread, not monotone)'
        ELSE 'no-signal (sign/shape fail)'
    END AS shape_verdict
FROM shaped s
ORDER BY s.exchange, s.symbol, s.h_secs;
