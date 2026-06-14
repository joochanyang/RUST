# Phase 6 Live Readiness

## Implemented

- Live startup is blocked unless all readiness checks are passed in `live_readiness_checks`.
- Readiness API:
  - `GET /api/live-readiness`
  - `POST /api/live-readiness/checks`
  - `POST /api/live-readiness/paper-trading-14d/run`
- Backtest evidence API:
  - `POST /api/backtest-runs`
- Backtest runner API:
  - `POST /api/backtest-runs/run`
- Failure injection evidence API:
  - `POST /api/failure-injection-runs`
- Failure injection execution API:
  - `POST /api/failure-injection-runs/run`
- Dashboard displays live readiness approval and missing gates.
- Readiness/backtest/failure-injection POST endpoints share dashboard control-token protection when configured.
- Dashboard browser requests go through same-origin Next.js route handlers. The Rust API control token stays server-side, and mutating dashboard requests require CSRF validation.
- Dashboard user/session authentication is implemented for non-local exposure when `DASHBOARD_PASSWORD` is configured.
- Runbook includes backtesting, live readiness, failure injection, operator controls, and rollback.
- Backtest runner reads historical candles from PostgreSQL, applies the technical RSI/Bollinger strategy with project risk defaults, computes PnL/drawdown/trade metrics, and records the result in `backtest_runs`.
- Binance USD-M historical candle importer can populate restart-safe 1m candle datasets for long-range backtests using the official `/fapi/v1/klines` paging limit.
- Full BTCUSDT/ETHUSDT 1m historical import and DB-backed backtest evidence is recorded:
  - Backtest run `ecce24f5-61fc-46fc-85b5-6327bb34341b`
  - Period `2023-06-13T00:00:00Z` to `2026-06-12T23:28:00Z`
  - `3,156,416` candles loaded, `8,492` trades, max drawdown pct `8.299433420994645791951936090`
  - `backtest_mdd` readiness check recorded as passed.
- Failure injection execution evidence is recorded:
  - `killswitch_panic_close` locked runtime, closed open positions, and recorded `killswitch_verified`.
  - `exchange_429_rate_limit` simulated rate-limit recovery by locking runtime for operator review and recorded `rate_limit_recovery`.
- Paper-trading 14-day verifier is implemented and refuses to record readiness until the DB evidence span reaches at least 14 days with paper orders, fills, positions, and protection orders.

## Required Live Gates

- `backtest_mdd`
- `paper_trading_14d`
- `api_key_restricted`
- `killswitch_verified`
- `rate_limit_recovery`

## Policy

Live exchange order routing remains intentionally unimplemented. Because no live execution runtime exists yet, the process **refuses to start** in `live` mode: even with `LIVE_TRADING_APPROVED=true` and all readiness checks `passed`, startup runs the readiness gate and then `bail!`s with "live trading runtime is not implemented". This prevents an "approved live" process that would silently place no real orders (or fall back to the paper loop). When a live execution path is implemented, replace the bail in `main.rs` with the live runtime spawn.

## Remaining Work

1. Run at least 2 weeks of paper trading and record evidence.
2. Verify live exchange API keys are restricted to the minimum required permissions and record `api_key_restricted` evidence.
