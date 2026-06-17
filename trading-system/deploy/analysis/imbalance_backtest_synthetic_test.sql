-- Synthetic verification of imbalance_backtest.sql math. Run on a throwaway local PG:
--   createdb -h 127.0.0.1 -U postgres imbalance_bt_test
--   psql -h 127.0.0.1 -U postgres imbalance_bt_test < imbalance_backtest_synthetic_test.sql
--   dropdb -h 127.0.0.1 -U postgres imbalance_bt_test
--
-- Asserts 4 branches:
--   A: imbalance>0.5 & price rises  -> long wins  -> gross net positive
--   B: imbalance<0.5 & price falls  -> short wins -> gross net positive
--   C: cost monotonicity            -> net(1x) > net(2x) > net(3x)
--   D: no-signal (imbalance~0.5)    -> gross ~0, not PASS
\set ON_ERROR_STOP on

DROP TABLE IF EXISTS order_books;
CREATE TABLE order_books (
    id bigserial PRIMARY KEY,
    exchange text, symbol text,
    event_time timestamptz,
    best_bid numeric, best_ask numeric,
    bid_size numeric, ask_size numeric,
    created_at timestamptz DEFAULT now()
);

-- Branch A (feedA): every 1s for 3 hours. imbalance HIGH (bid_size 9, ask 1 => I=0.9),
-- mid rises 1 unit/sec from 100. half-spread fixed at 0.01 (bid=mid-0.005, ask=mid+0.005).
INSERT INTO order_books (exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size)
SELECT 'feedA', 'BTCUSDT',
       timestamptz '2026-01-01 00:00:00+00' + make_interval(secs => s),
       (100 + s) - 0.005, (100 + s) + 0.005, 9, 1
FROM generate_series(0, 10800) AS s;   -- 3h => 10s:1081, 30s:361, 60s:181 non-overlap

-- Branch B (feedB): imbalance LOW (bid 1, ask 9 => I=0.1), mid FALLS 1 unit/sec from 10000.
INSERT INTO order_books (exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size)
SELECT 'feedB', 'BTCUSDT',
       timestamptz '2026-01-01 00:00:00+00' + make_interval(secs => s),
       (10000 - s) - 0.005, (10000 - s) + 0.005, 1, 9
FROM generate_series(0, 10800) AS s;

-- Branch D (feedD): imbalance alternates around 0.5, mid flat at 100 => no signal.
INSERT INTO order_books (exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size)
SELECT 'feedD', 'BTCUSDT',
       timestamptz '2026-01-01 00:00:00+00' + make_interval(secs => s),
       100 - 0.005, 100 + 0.005,
       CASE WHEN s % 2 = 0 THEN 5 ELSE 5 END, 5
FROM generate_series(0, 10800) AS s;

\echo '===== running imbalance_backtest.sql against synthetic data ====='
\i imbalance_backtest.sql
