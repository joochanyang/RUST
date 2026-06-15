# Capture-only deployment (Hetzner)

Runs `trading-api` in **capture-only** mode: ingests + 1s-downsamples order-book /
candle market data into a dedicated Postgres, places **no orders**. This is the
data-collection step for the order-book-imbalance hypothesis (needs weeks of 1s
top-of-book before any pre-registered analysis).

## Why capture-only is no-trade

`TRADING_MODE=paper` + `PAPER_TRADING_ENABLED=false` + `BINANCE_TESTNET_ENABLED=false`
⇒ neither the paper nor the testnet order loop is spawned (verified in
`crates/api/src/main.rs`); market events only persist to the DB. The compose file
already sets these.

## Disk

`order_books` is append-only. At full resolution it would fill Hetzner's free
disk (~63G as of 2026-06-16) in ~2 weeks. `MARKET_DATA_ORDERBOOK_SAMPLE_SECS=1`
keeps one row per (exchange, symbol) per second (~50–100× less), so a multi-month
capture fits. Still monitor disk and the insert rate.

## Deploy on Hetzner (5.161.112.248, x86_64)

`docker build` works on this host (the SSH-build credsStore trap is home-server
only). The image must be built on the host because the dev machine is arm64.

```sh
# 1. get the code on the host (clone from the git remote — do NOT rsync the
#    working tree; pushing a private tree sideways is blocked as exfiltration)
git clone git@github.com:joochanyang/RUST.git ~/RUST   # or https with a PAT
cd ~/RUST/trading-system    # git root is the parent; the crate is in trading-system/

# 2. set the dedicated capture DB password (used by both services)
export CAPTURE_DB_PASSWORD='<choose-a-strong-password>'

# 3. build + start (dedicated trading-capture-postgres + trading-capture)
docker compose -f deploy/docker-compose.capture.yml up -d --build

# 4. watch it come up (migrations run on startup via RUN_MIGRATIONS=true)
docker compose -f deploy/docker-compose.capture.yml logs -f capture
```

## Health / monitoring

```sh
# in-container health endpoint (not exposed externally by design)
docker compose -f deploy/docker-compose.capture.yml exec capture \
  wget -qO- http://127.0.0.1:8080/api/health

# insert rate + freshness (a collapse = a stall the staleness reconnect should
# self-heal; if not, restart the capture service)
docker compose -f deploy/docker-compose.capture.yml exec capture-postgres \
  psql -U trading -d trading_system -c \
  "SELECT exchange, symbol, count(*), max(event_time) FROM order_books GROUP BY 1,2 ORDER BY 1,2;"
```

## When enough data has accumulated (weeks later)

Pre-register the imbalance analysis FIRST (does `bid_size/(bid_size+ask_size)`
predict the next N-minute return? IC / quantile spread), then — only if a signal
survives — implement a strategy and run it through the same walk-forward + OOS +
fee + adversarial-review discipline as the (falsified) price-direction families.
Same family-wise overfitting guard applies.
