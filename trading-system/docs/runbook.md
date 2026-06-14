# Runbook

## Start

1. Copy `configs/example.env` to `.env`.
2. Set `DATABASE_URL` to a PostgreSQL database dedicated to this system.
3. Start the backend with `cargo run -p trading-api`.
4. Confirm `GET /api/health` returns `status: ok` and `database: ok`.

For the dashboard, set `API_BASE_URL` and `DASHBOARD_CONTROL_TOKEN` in `dashboard/.env.local`. Do not expose the dashboard control token through `NEXT_PUBLIC_*` variables. Before exposing the dashboard outside localhost, also set `DASHBOARD_PASSWORD` and a high-entropy `DASHBOARD_SESSION_SECRET`; the Next.js proxy rejects dashboard API requests without a valid signed session cookie.

## Stop

Send `Ctrl-C` to the API process. The Axum server uses graceful shutdown.

## Locked Mode

Set `TRADING_MODE=locked` before startup to block new entries while preserving read-only API visibility.

## Binance Testnet Mode

Use Binance Futures testnet only with testnet-issued API keys:

```sh
TRADING_MODE=testnet
MARKET_DATA_ENABLED=true
MARKET_DATA_EXCHANGES=binance
MARKET_DATA_SYMBOLS=BTCUSDT,ETHUSDT
BINANCE_TESTNET_ENABLED=true
BINANCE_TESTNET_API_KEY=<testnet-key>
BINANCE_TESTNET_API_SECRET=<testnet-secret>
BINANCE_TESTNET_MAX_ORDER_NOTIONAL=50
```

In testnet mode, the strategy still uses public WebSocket market data and the same AI/risk gates. Orders are sent to Binance USD-M Futures testnet REST API. `BINANCE_TESTNET_MAX_ORDER_NOTIONAL` caps the notional value of each testnet order.

If protection order placement fails after a testnet market entry, the runtime switches to `locked` and emits a Telegram notification when Telegram is enabled.

## Telegram Remote Control

Enable Telegram notifications and restricted commands:

```sh
TELEGRAM_ENABLED=true
TELEGRAM_BOT_TOKEN=<bot-token>
TELEGRAM_ALLOWED_CHAT_ID=<your-chat-id>
TELEGRAM_NOTIFY_CHAT_ID=<notification-chat-id>
```

Supported commands:

- `/status`
- `/readiness`
- `/lock`
- `/unlock`
- `/panic_close`
- `/help`

Commands from any chat other than `TELEGRAM_ALLOWED_CHAT_ID` are ignored. Telegram tokens must never be committed to the repository.

## Live Mode Gate

`TRADING_MODE=live` is rejected unless `LIVE_TRADING_APPROVED=true` is also present. Live API keys must never be committed to the repository.

The backend also checks `live_readiness_checks` before live startup. Required checks:

- `backtest_mdd`
- `paper_trading_14d`
- `api_key_restricted`
- `killswitch_verified`
- `rate_limit_recovery`

## Live Readiness Recording

Record evidence through the API after independent verification:

```sh
curl -X POST http://127.0.0.1:8080/api/live-readiness/checks \
  -H 'content-type: application/json' \
  -d '{"check_key":"killswitch_verified","status":"passed","evidence":{"dashboard_lock":true},"verified_by":"operator"}'
```

Check approval state:

```sh
curl http://127.0.0.1:8080/api/live-readiness
```

Verify whether paper trading has accumulated enough DB evidence for the 14-day gate:

```sh
curl -X POST http://127.0.0.1:8080/api/live-readiness/paper-trading-14d/run \
  -H 'x-dashboard-control-token: <token-if-configured>'
```

This endpoint only records `paper_trading_14d` when paper-mode orders, fills, positions, and protection orders exist and the observed DB span is at least 14 days. Otherwise it returns `409 Conflict` with the current evidence summary.

## Backtesting

Run a DB-backed backtest over stored candles:

```sh
curl -X POST http://127.0.0.1:8080/api/backtest-runs/run \
  -H 'content-type: application/json' \
  -d '{
    "exchange":"binance",
    "symbols":["BTCUSDT","ETHUSDT"],
    "timeframe":"1m",
    "period_start":"2023-01-01T00:00:00Z",
    "period_end":"2026-01-01T00:00:00Z",
    "initial_equity":"10000",
    "daily_loss_limit":"500"
  }'
```

The runner records metrics in `backtest_runs`. Review `max_drawdown_pct`, trade count, and symbol coverage before recording the `backtest_mdd` readiness check.

Import Binance USD-M historical candles before long-range backtests:

```sh
DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/trading_system \
HISTORICAL_SYMBOLS=BTCUSDT,ETHUSDT \
HISTORICAL_TIMEFRAME=1m \
HISTORICAL_START=2023-06-13 \
HISTORICAL_END=2026-06-13 \
cargo run -p trading-api --bin import_historical_candles
```

The importer is restart-safe: candles are keyed by `(exchange, symbol, timeframe, open_time)` and upserted on conflict.

## Failure Injection

Record forced failure tests before live mode:

```sh
curl -X POST http://127.0.0.1:8080/api/failure-injection-runs/run \
  -H 'content-type: application/json' \
  -d '{"scenario":"killswitch_panic_close"}'

curl -X POST http://127.0.0.1:8080/api/failure-injection-runs/run \
  -H 'content-type: application/json' \
  -d '{"scenario":"exchange_429_rate_limit"}'
```

The execution endpoint records `failure_injection_runs` rows and updates the corresponding live-readiness checks when the simulated control action passes. Manual evidence can still be recorded when an external test cannot be executed by the API:

```sh
curl -X POST http://127.0.0.1:8080/api/failure-injection-runs \
  -H 'content-type: application/json' \
  -d '{"scenario":"exchange_429","expected_action":"locked","observed_action":"locked","passed":true,"details":{}}'
```

## Operator Controls

- Lock new entries: `POST /api/control/lock`
- Unlock back to paper mode: `POST /api/control/unlock`
- Panic close all open paper positions and lock: `POST /api/control/panic-close`
- Close one paper position: `POST /api/positions/:id/close`
- Acknowledge risk event: `POST /api/risk-events/:id/ack`

## Rollback

Stop the API process, keep `TRADING_MODE=locked` or `paper`, and restore the prior database backup if a migration or operational action must be rolled back. Do not restart in `live` until readiness checks are reverified.
