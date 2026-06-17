-- imbalance_backtest.sql — fixed-H-second holding backtest of the orderbook imbalance signal.
--
-- Pre-registration: docs/superpowers/specs/2026-06-16-orderbook-imbalance-preregistration.md
-- Design:           docs/superpowers/specs/2026-06-18-imbalance-backtest-design.md
--
-- Locked discipline (inherited from imbalance_fast.sql / imbalance_ic.sql — do NOT change):
--   * mid = (best_bid+best_ask)/2 ; returns on mid, never on bid/ask (bounce trap)
--   * NON-OVERLAPPING base samples: one entry every H seconds (epoch_second % H = 0)
--   * forward/exit = first tick in [t+H, t+H+2s); NO interpolation; unmatched entry dropped
--   * per (exchange, symbol, horizon); NO pooling
--   * sample-count floor = 2000 non-overlapping entries before any PASS/FAIL judgment
--
-- Money model (design sec 3):
--   direction   = sign(imbalance - 0.5)            (>0.5 long, <0.5 short)
--   gross_bps   = direction * (exit_mid/entry_mid - 1) * 10000
--   half_spread = (best_ask-best_bid)/2 / mid * 10000   (measured at ENTRY)
--   taker_bps   = 5.0   (per side, conservative)
--   roundtrip_cost(k) = k * 2 * (taker_bps + half_spread_bps)   for k in {1,2,3}
--   net_bps(k)  = gross_bps - roundtrip_cost(k)
--
-- NOTE: temp tables are session-scoped — run this whole file on ONE connection
--       (psql < file, or a single sqlx connection batch).

\set ON_ERROR_STOP on
\timing on

DROP TABLE IF EXISTS _ticks;
CREATE TEMP TABLE _ticks AS
SELECT exchange, symbol,
       date_trunc('second', event_time)                         AS ts,
       (best_bid + best_ask) / 2.0                              AS mid,
       bid_size / (bid_size + ask_size)                         AS imbalance,
       (best_ask - best_bid) / 2.0 / ((best_bid + best_ask)/2.0) * 10000 AS half_spread_bps
FROM (
    SELECT exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size,
           row_number() OVER (PARTITION BY exchange, symbol, date_trunc('second', event_time)
                              ORDER BY event_time) AS rn
    FROM order_books
    WHERE bid_size + ask_size > 0
      AND best_bid > 0 AND best_ask > 0
) d
WHERE rn = 1;

CREATE INDEX _ticks_idx ON _ticks (exchange, symbol, ts) INCLUDE (mid);
ANALYZE _ticks;

-- entries (non-overlapping) + fixed-H exit via index seek
DROP TABLE IF EXISTS _trades;
CREATE TEMP TABLE _trades AS
WITH horizons(h_secs) AS (VALUES (10),(30),(60)),
entries AS (
    SELECT t.exchange, t.symbol, h.h_secs, t.ts,
           t.mid AS entry_mid, t.imbalance, t.half_spread_bps
    FROM _ticks t CROSS JOIN horizons h
    WHERE (extract(epoch FROM t.ts)::bigint % h.h_secs) = 0
      AND t.mid > 0
      AND t.imbalance <> 0.5            -- no direction => no trade
)
SELECT e.exchange, e.symbol, e.h_secs,
       e.half_spread_bps,
       sign(e.imbalance - 0.5)
         * (x.mid / e.entry_mid - 1.0) * 10000   AS gross_bps
FROM entries e
JOIN LATERAL (
    SELECT t2.mid FROM _ticks t2
    WHERE t2.exchange = e.exchange AND t2.symbol = e.symbol
      AND t2.ts >= e.ts + make_interval(secs => e.h_secs)
      AND t2.ts <  e.ts + make_interval(secs => e.h_secs) + make_interval(secs => 2.0)
    ORDER BY t2.ts LIMIT 1
) x ON TRUE;

ANALYZE _trades;

\echo '===== IMBALANCE BACKTEST: net return (bps) after roundtrip cost ====='
WITH agg AS (
    SELECT exchange, symbol, h_secs,
           count(*)                              AS n,
           avg(gross_bps)                        AS mean_gross_bps,
           avg(half_spread_bps)                  AS mean_half_spread_bps,
           avg(2.0 * (5.0 + half_spread_bps))    AS mean_roundtrip_1x_bps
    FROM _trades GROUP BY exchange, symbol, h_secs
)
SELECT exchange, symbol, h_secs AS horizon_s, n AS samples,
       round(mean_gross_bps::numeric, 4)                                   AS gross_bps,
       round(mean_half_spread_bps::numeric, 4)                             AS half_spread_bps,
       round((mean_gross_bps - mean_roundtrip_1x_bps)::numeric, 4)         AS net_1x_bps,
       round((mean_gross_bps - 2*mean_roundtrip_1x_bps)::numeric, 4)       AS net_2x_bps,
       round((mean_gross_bps - 3*mean_roundtrip_1x_bps)::numeric, 4)       AS net_3x_bps,
       CASE
           WHEN n < 2000 THEN 'INCONCLUSIVE (n<floor)'
           WHEN (mean_gross_bps - mean_roundtrip_1x_bps) <= 0 THEN 'FAIL (net<=0 @1x)'
           WHEN (mean_gross_bps - 2*mean_roundtrip_1x_bps) <= 0 THEN 'MARGINAL (dies @2x)'
           ELSE 'net+ @1x (single-regime; needs WFO+OOS)'
       END AS verdict
FROM agg ORDER BY exchange, symbol, h_secs;

\echo '===== sign-consistency cross-check: avg(direction*fwd) should be > 0 (matches IC sign) ====='
SELECT exchange, symbol, h_secs AS horizon_s,
       round(avg(gross_bps)::numeric, 4) AS avg_signed_bps,
       CASE WHEN avg(gross_bps) > 0 THEN 'OK (+)' ELSE 'SIGN MISMATCH' END AS check
FROM _trades GROUP BY exchange, symbol, h_secs ORDER BY exchange, symbol, h_secs;
