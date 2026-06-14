# Trading Dashboard

Next.js App Router dashboard for the Rust trading backend.

```sh
cp .env.example .env.local
npm install
npm run dev
```

The dashboard proxies browser requests through same-origin Next.js route handlers. Set `API_BASE_URL` to the Rust API origin and keep `DASHBOARD_CONTROL_TOKEN` server-side only. Mutating dashboard requests require a same-origin CSRF token.

Set `NEXT_PUBLIC_DASHBOARD_WS_URL` to the Rust `/ws/dashboard` endpoint to receive real-time account, performance, position, trade history, and risk-event snapshots. If the WebSocket URL is omitted or the connection closes, the dashboard falls back to polling the same-origin API routes.

Set `DASHBOARD_PASSWORD` and `DASHBOARD_SESSION_SECRET` before exposing the dashboard outside localhost. When `DASHBOARD_PASSWORD` is configured, the page and proxied `/api/*` requests require a signed HTTP-only session cookie.
