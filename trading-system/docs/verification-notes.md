# Verification Notes

## Verified

Rust workspace verification completed successfully after generating `Cargo.lock`:

```sh
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
```

Latest focused regression after market-latency and paper-risk-state updates:

```sh
cargo fmt --all --check
cargo test -p trading-api
```

Latest focused regression after operator-lock, restart-safe TP/SL, operator-close PnL, and CORS hardening updates:

```sh
cargo fmt --all --check
cargo check --workspace
cargo test -p trading-api
cargo test -p trading-execution
TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/trading_system cargo test -p trading-api paper_trading::tests::persists_signal_ai_and_protected_paper_order_when_database_is_configured -- --nocapture
```

Latest focused regression after Binance testnet and Telegram remote-control implementation:

```sh
cargo fmt --all --check
cargo check --workspace
cargo test -p trading-exchange
cargo test -p trading-api
```

Latest focused regression after focused performance dashboard implementation:

```sh
cargo fmt --all --check
cargo check --workspace
cargo test -p trading-api
cd dashboard
npm run build
```

Latest focused regression after dashboard session authentication:

```sh
cd dashboard
npm run build
```

Latest focused regression after WebSocket dashboard snapshot expansion:

```sh
cargo fmt --all --check
cargo check --workspace
cargo test -p trading-api
cd dashboard
npm run build
```

Latest focused regression after exchange order-safety fixes (lot/tick rounding, fill reporting, live-mode guard):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets   # no new warnings vs. pre-existing baseline
TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/trading_system_test cargo test --workspace
# 57 passed, 0 failed (was 50; +7 new exchange tests)
```

Exchange order-safety fixes verified in this pass:

- **H1 — orders are rounded to exchange lot/tick increments before sending.** `crates/exchange/src/binance.rs` adds `SymbolFilters` (step/tick/min-qty/min-notional), `round_quantity` (truncate down to `stepSize` — never larger than intended), `round_protection_price` (direction-aware tick rounding — see below), `is_tradeable` (min-qty + min-notional), and `fetch_symbol_filters` (parses public `/fapi/v1/exchangeInfo`). `crates/api/src/testnet_runtime.rs` fetches filters once at startup (locks the runtime and skips all orders if the fetch fails), rounds quantity + stop/take prices before each order, rounds the reduce-only flatten quantity, and skips signals below the exchange minimums. Previously `quantity = notional/price` was sent unrounded and Binance rejected it with `-1111`.
  - **Direction-aware protection-price rounding** (per adversarial review): a single blind truncate-down was asymmetric — it pulled a SHORT stop and a LONG take toward entry. `round_protection_price` now rounds LONG protection prices DOWN (floor) and SHORT protection prices UP (ceil), so the stop never moves toward entry (no weaker protection) and the take never moves away (stays reachable). Bounded to ≤1 tick either way.
  - `rounds_quantity_down_to_step_size`, `long_protection_prices_round_down_to_tick`, `short_protection_prices_round_up_to_tick`, `protection_price_on_exact_tick_is_unchanged`, `truncate_with_zero_increment_is_identity`, `is_tradeable_enforces_min_qty_and_min_notional`, `parses_exchange_info_filters`.
- **H2 — fills are never overstated.** `order_ack_from_value` no longer falls back to `origQty` when `executedQty` is absent; a NEW/partial order now reports `executed_quantity = 0` instead of the ordered quantity.
  - `order_ack_uses_executed_quantity_when_present`, `order_ack_does_not_treat_ordered_quantity_as_filled` (RED against pre-fix code).
- **H3 — live mode refuses to boot.** `crates/api/src/main.rs` runs the readiness gate and then `bail!`s in `live` mode (no live execution runtime exists), instead of starting a process that looks "approved live" but places no real orders.

Latest focused regression after money-correctness fixes (mark-to-market persistence, idempotent close, testnet naked-position flatten):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets   # no new warnings vs. pre-existing baseline
TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/trading_system_test cargo test --workspace
# 49 passed, 0 failed (was 42; +7 new tests)
```

Money-correctness fixes verified in this pass:

- **C1 — open positions are now marked to market.** `update_open_position_marks` (`crates/api/src/execution_repository.rs`) updates `mark_price`/`unrealized_pnl` for open positions on every order-book tick (mid price = `(best_bid + best_ask) / 2`) from `crates/api/src/strategy_runtime.rs`. Without this, every non-SL/TP close realized ~0 PnL and the daily-loss kill switch never saw unrealized losses.
  - `manual_close_realizes_marked_pnl_after_mark_update`: a long opened at 50000 and marked to 49000 closes at `realized_pnl = -1000`, not 0.
  - `account_risk_state_reflects_marked_unrealized_pnl`: open unrealized PnL flows into account equity.
- **C2 — close is idempotent against out-of-band closes (both paths).** Both close paths now guard on `closed_at IS NULL` and only record an exit when they actually closed the position:
  - `persist_paper_exit` (tracker path): closes with `WHERE id = $ AND closed_at IS NULL`, checks `rows_affected`, returns `Result<bool>` (`false` = already closed, transaction rolled back, no duplicate exit).
  - `close_position_for_exit` (bulk dashboard/panic path, used by `close_open_paper_positions`): same guard, returns `bool`; the bulk loop only inserts the exit when the guarded close affected a row. This makes the bulk path idempotent on its own rather than relying solely on the `SELECT … FOR UPDATE` row lock, closing the verified PnL-double-counting race surfaced by adversarial review.
  - `tracker_exit_is_idempotent_against_out_of_band_close`: after a dashboard/panic close, a tracker exit for the same position records no second `paper_exits` row. Confirmed RED against the pre-fix tracker code.
  - `bulk_close_is_idempotent_on_repeat`: closing the same position twice via the bulk path closes once (returns 1) then no-ops (returns 0) with exactly one `paper_exits` row.
- **C3 — testnet protection failure flattens the naked position.** On `place_protection_orders` failure, `crates/api/src/testnet_runtime.rs` locks the runtime, calls `flatten_position` (reduce-only opposite-side market order), records a `critical` `risk_event`, sends a CRITICAL alert, does not register `open_position_keys`, and does not send the phantom-SL/TP success notification. `MarketOrderRequest` gained `reduce_only: bool` (`crates/exchange/src/lib.rs`), wired into `crates/exchange/src/binance.rs` (`reduceOnly=true` only when set).
  - `flatten_sends_reduce_only_opposite_side_order`, `flatten_closes_short_with_buy`, `flatten_surfaces_failure`.
- **AI gate fail-closed (review false alarm).** The original review flagged `crates/ai/src/lib.rs` as allowing a missing pattern under `fail_closed`; on re-reading, lines that match `None if self.config.fail_closed` already Block correctly. No code change; a regression test (`enabled_fail_closed_gate_blocks_missing_pattern_when_macro_present`) locks the behavior.

Note: an isolated `trading_system_test` database was created (idempotently, no DROP) so DB integration tests run for real without touching the dev database.

Dashboard verification completed successfully after generating `dashboard/package-lock.json`:

```sh
cd dashboard
npm install
npm audit
npm run build
npm run dev
```

Dashboard browser smoke test:

- `http://127.0.0.1:3000/` renders the dashboard shell.
- Lock, Unlock, and Panic Close controls are present.
- Browser console had no warnings or errors during the smoke test.

Dashboard dependency hardening:

- Next.js was upgraded to `16.2.9`.
- React and React DOM were upgraded to `19.2.7`.
- React type packages were upgraded to React 19-compatible versions.
- `postcss` is pinned through npm `overrides` to avoid the vulnerable transitive version bundled by Next.
- `npm audit` reports `0 vulnerabilities`.

Test coverage currently exercised:

- Core enum/string normalization and trading-mode defaults.
- Exchange payload parsing for Binance, Bybit, and Bitget market streams.
- Strategy signal behavior for insufficient history and extreme RSI/Bollinger conditions.
- Risk sizing and locked-account blocking.
- Paper broker live-order rejection and protected paper-fill creation.
- Paper TP/SL tracking.
- Market data latency above 2 seconds records a `market_data_latency` risk event payload.
- Paper strategy risk state is rebuilt from stored PnL and open-position data.
- Runtime lock blocks new paper entries.
- Open protected paper orders are restored into the TP/SL tracker after API restart.
- Manual close and panic close record realized `paper_exits`.
- Backend CORS is limited to local dashboard origins instead of permissive origins.
- `TRADING_MODE=testnet` starts a Binance Futures testnet strategy loop when testnet credentials are configured.
- Binance testnet signed REST requests use HMAC-SHA256 signatures and the official testnet base URL.
- Telegram notifications and restricted remote-control commands are implemented with long polling.
- AI entry gate allow/block behavior.
- API settings parsing and candle-buffer runtime behavior.
- Health endpoint database check with PostgreSQL `bigint` scalar.
- Binance Futures WebSocket routing through `stream.binancefuture.com`, including kline and book ticker stream URL construction.
- DB-backed backtest runner over stored candles with PnL, drawdown, signal, and trade metrics.
- Restart-safe Binance USD-M historical candle importer for populating long-range 1m datasets before full backtests.
- DB-backed paper execution persistence for signal, AI decisions, order, fill, position, protection order, take-profit paper exit, position close, and protection status update.
- Dashboard production build and TypeScript validation.
- Dashboard control token rejection for protected POST endpoints.
- Open paper positions are marked to market on order-book ticks; manual/panic closes realize the marked (non-zero, correctly-signed) PnL.
- Account risk state equity includes live unrealized PnL from marked open positions.
- Paper exit persistence is idempotent: a tracker exit for a position already closed out-of-band records no duplicate `paper_exits` row.
- Testnet protection-order failure flattens the position with a reduce-only opposite-side market order and escalates as a critical risk event instead of reporting a successful entry.
- Order quantity is truncated down to the exchange `stepSize`; protection prices are rounded direction-aware (LONG down, SHORT up) to `tickSize`; orders below `minQty`/`minNotional` are skipped. `exchangeInfo` filters are parsed and cached at testnet startup.
- Order acknowledgements report only the actually-executed quantity (never the ordered quantity) when `executedQty` is absent.
- Live mode refuses to start because no live execution runtime is implemented.

Token protection was verified with `DASHBOARD_CONTROL_TOKEN=test-token`:

- `POST /api/control/lock` returns `401` without `x-dashboard-control-token`.
- `POST /api/live-readiness/checks` returns `401` without `x-dashboard-control-token`.
- `POST /api/failure-injection-runs` returns `401` without `x-dashboard-control-token`.

## Database Runtime Verification

The local `trading_system` PostgreSQL database was created and verified through a temporary sqlx admin tool because `psql` and `createdb` hung in this desktop environment.

Verified:

- PostgreSQL is listening on `127.0.0.1:5432`.
- `RUN_MIGRATIONS=true` creates all 13 required application tables:
  `ai_decisions`, `backtest_runs`, `candles`, `failure_injection_runs`, `live_readiness_checks`, `order_books`, `order_fills`, `orders`, `paper_exits`, `positions`, `protection_orders`, `risk_events`, `signals`.
- `GET /api/health` returns `200` with `{"status":"ok","database":"ok","mode":"paper","service":"trading-api"}`.
- `GET /api/account`, `/api/positions`, `/api/signals`, `/api/risk-events`, and `/api/live-readiness` return DB-backed `200` responses.
- `MARKET_DATA_ENABLED=true MARKET_DATA_EXCHANGES=binance,bybit,bitget MARKET_DATA_SYMBOLS=BTCUSDT` inserts BTCUSDT rows into both `candles` and `order_books` for Binance, Bybit, and Bitget.
- `POST /api/backtest-runs/run` returns `201` with computed metrics and records a row in `backtest_runs`.
- Binance historical import completed for BTCUSDT/ETHUSDT 1m candles from `2023-06-13T00:00:00Z` through the latest available common end, with no missing rows between each symbol's imported min/max:
  - BTCUSDT: `1,578,208` rows, `2023-06-13T00:00:00Z` to `2026-06-12T23:27:00Z`.
  - ETHUSDT: `1,578,218` rows, `2023-06-13T00:00:00Z` to `2026-06-12T23:37:00Z`.
- Full DB-backed backtest run `ecce24f5-61fc-46fc-85b5-6327bb34341b` completed over BTCUSDT/ETHUSDT from `2023-06-13T00:00:00Z` to `2026-06-12T23:28:00Z`: `3,156,416` candles loaded, `8,492` trades, `187,168` signals seen, final equity `9431.790976752926722338072377`, realized PnL `-568.209023247073277661927623`, max drawdown pct `8.299433420994645791951936090`.
- `backtest_mdd` live-readiness evidence was recorded from that full backtest run; live approval remains blocked by the remaining gates.
- `POST /api/failure-injection-runs/run` executed `killswitch_panic_close`, locked runtime, closed `1` open position, recorded a passing failure-injection row, and recorded `killswitch_verified`.
- `POST /api/failure-injection-runs/run` executed `exchange_429_rate_limit`, locked runtime for operator review, recorded a passing failure-injection row, and recorded `rate_limit_recovery`.
- `POST /api/live-readiness/paper-trading-14d/run` correctly returns `409 Conflict` with current evidence instead of recording readiness when the observed paper-trading DB span is below 14 days. Current evidence at verification time: `3` paper orders, `3` fills, `3` positions, `3` protection orders, `2` paper exits, observed span `0` days.
- Current live readiness has passed checks: `backtest_mdd`, `killswitch_verified`, and `rate_limit_recovery`; it remains unapproved until `paper_trading_14d` and `api_key_restricted` are recorded.
- `TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/trading_system cargo test -p trading-api paper_trading::tests::persists_signal_ai_and_protected_paper_order_when_database_is_configured -- --nocapture` passes.
- `/ws/dashboard` accepts a WebSocket connection and streams an account snapshot.
- `/ws/dashboard` streams a full dashboard snapshot with account, positions, performance, trade history, and risk events.
- Live startup fails with `live mode requires all live readiness checks to pass` when `TRADING_MODE=live LIVE_TRADING_APPROVED=true` is set but readiness checks are incomplete.

Dashboard-to-API runtime verification:

- With `API_BASE_URL=http://127.0.0.1:8080` and server-side `DASHBOARD_CONTROL_TOKEN=test-token`, the dashboard renders DB-backed account, performance, positions, trade history, and risk events through same-origin Next.js route handlers.
- Browser clicks on Lock, Unlock, and Panic Close call the API and update the rendered mode/reason.
- Browser console had no warnings or errors during DB-backed dashboard checks.
- With `TELEGRAM_ENABLED=false MARKET_DATA_ENABLED=false RUN_MIGRATIONS=true`, the Rust API and focused Next.js dashboard render DB-backed performance metrics, live positions, trade history, and risk events at `http://127.0.0.1:3000/`.
- Desktop browser verification found the expected Korean dashboard sections: `운영 대시보드`, `실시간 포지션`, `거래 내역`, and `리스크 기록`, with no error strip and no console warnings/errors.
- Mobile browser verification at a 390px viewport found no page-level horizontal overflow; tables remain inside scrollable table containers and the control buttons fit their grid cells.
- Dashboard CSRF/control-token hardening was verified:
  - `GET /api/live-readiness` through the dashboard proxy returns `200`.
  - `POST /api/control/lock` without CSRF returns `403`.
  - `POST /api/control/lock` with `/api/csrf` cookie/header returns `200` and is authorized with the server-side `DASHBOARD_CONTROL_TOKEN`.
  - Browser rendering at `http://127.0.0.1:3000` shows the focused operations dashboard, no error strip, and no console errors.
- Dashboard password/session authentication was verified with `DASHBOARD_PASSWORD=test-pass DASHBOARD_SESSION_SECRET=test-secret`:
  - `GET /api/account` through the dashboard proxy returns `401` without a session cookie.
  - `POST /api/session` returns `401` for an invalid password.
  - `POST /api/session` returns `200` and sets an HTTP-only signed session cookie for the valid password.
  - `GET /api/session` returns `{"authRequired":true,"authenticated":true}` when the signed session cookie is supplied.
  - Browser rendering at `http://127.0.0.1:3001` shows the login panel before authentication and switches to `운영 대시보드` after login, with no console warnings/errors.

## Verification Still Required

Operational readiness still requires real-world evidence collection before any live deployment:

- Run at least 2 weeks of paper trading and record evidence.
- Verify live exchange API keys are restricted to the minimum required permissions and record `api_key_restricted` evidence.
