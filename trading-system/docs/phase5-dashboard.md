# Phase 5 Dashboard and Operations

## Implemented

- Backend API routes:
  - `GET /api/account`
  - `GET /api/positions`
  - `GET /api/performance`
  - `GET /api/trade-history?limit=100`
  - `GET /api/signals?limit=100`
  - `GET /api/risk-events?limit=100`
  - `POST /api/control/lock`
  - `POST /api/control/unlock`
  - `POST /api/control/panic-close`
  - `POST /api/positions/:id/close`
  - `POST /api/risk-events/:id/ack`
- `GET /ws/dashboard`
- Server-side `DASHBOARD_CONTROL_TOKEN` protection for mutating dashboard/control APIs.
- Same-origin Next.js route handlers proxy browser requests to the Rust API without exposing the control token to browser JavaScript.
- CSRF validation for mutating dashboard requests.
- Optional dashboard password/session authentication. Set `DASHBOARD_PASSWORD` before exposing the dashboard outside localhost; signed HTTP-only session cookies are used after login.
- WebSocket dashboard snapshots include account, performance, positions, trade history, and risk events. The browser uses `NEXT_PUBLIC_DASHBOARD_WS_URL` when configured and falls back to API polling if the stream is unavailable.
- Runtime control state for paper/locked mode.
- Operator actions are recorded in `risk_events`.
- CORS layer for local dashboard -> backend calls.
- Next.js App Router dashboard under `dashboard/`.
- Dashboard views:
  - Performance summary: mode, net PnL, total entries, win rate, daily realized PnL, unrealized PnL, average/best/worst trade, take-profit count, and stop-loss count.
  - Live positions with entry, mark, quantity, unrealized PnL, take-profit, stop-loss, and manual close action.
  - Trade history with per-entry realized PnL, exit price, exit trigger, and open/closed status.
  - Risk events with acknowledge action.
  - Operator controls: refresh, lock, unlock, and panic close.

## Environment

Dashboard:

```sh
API_BASE_URL=http://127.0.0.1:8080
NEXT_PUBLIC_DASHBOARD_WS_URL=ws://127.0.0.1:8080/ws/dashboard
DASHBOARD_CONTROL_TOKEN=
DASHBOARD_PASSWORD=
DASHBOARD_SESSION_SECRET=
```

Backend:

```sh
DASHBOARD_CONTROL_TOKEN=
```

When `DASHBOARD_CONTROL_TOKEN` is non-empty, mutating Rust API endpoints require the `x-dashboard-control-token` header. The dashboard keeps that token server-side in Next.js route handlers. Browser mutations must include a valid CSRF token from `/api/csrf`.

When `DASHBOARD_PASSWORD` is non-empty, the dashboard page and same-origin `/api/*` proxy require a signed session cookie. Set a high-entropy `DASHBOARD_SESSION_SECRET` in production; if it is omitted, the password is used as the signing secret.

## Remaining Work

1. Persist Binance testnet order/position/exits if testnet trading performance must be included in the same dashboard metrics as paper trading.
