#!/usr/bin/env bash
# Watchdog for the capture-only deployment (see deploy/README.md).
#
# The compose `restart: unless-stopped` policy recovers a crashed container, and
# the per-stream staleness reconnect recovers a stalled WebSocket — but neither
# covers: a container stuck in a non-crash bad state, a postgres that stopped,
# or disk filling. For a capture meant to run unattended for weeks, this cron
# watchdog is the safety net.
#
# Actions (every run, in order):
#   1. If either container is not running -> `compose up -d` (idempotent).
#   2. If order_books has not advanced in > STALL_SECS -> restart capture
#      (a real stall the in-process reconnect failed to clear).
#   3. If the root filesystem is > DISK_WARN_PCT full -> log a WARNING only
#      (never auto-delete capture data; pruning is a human decision).
#   4. Hourly: check the imbalance data-sufficiency gate (deploy/analysis/README.md);
#      log GATE ready once all 18 feed×horizon clear the 2000-sample floor, else the
#      progress count. Log-only — running the analysis is a human decision (pre-reg).
#
# No-op and silent-ish when healthy (one OK line per run). Install via cron:
#   */5 * * * * /opt/trading-capture/watchdog.sh >> /var/log/trading-capture-watchdog.log 2>&1
set -euo pipefail

PROJECT_DIR="${PROJECT_DIR:-/root/RUST/trading-system}"
COMPOSE_FILE="deploy/docker-compose.capture.yml"
ENV_FILE=".env"
CAPTURE_CONTAINER="trading-capture"
PG_CONTAINER="trading-capture-postgres"
STALL_SECS="${STALL_SECS:-180}"      # healthy freshness is <1s; 180s = real stall
DISK_WARN_PCT="${DISK_WARN_PCT:-85}"
GATE_FLOOR="${GATE_FLOOR:-2000}"     # non-overlapping sample floor per feed×horizon (pre-reg §5)
GATE_CHECK_SECS="${GATE_CHECK_SECS:-3600}"   # throttle the heavier gate query to hourly
GATE_STAMP="${GATE_STAMP:-/tmp/trading-capture-gate.stamp}"   # last gate-check time
GATE_READY_LATCH="${GATE_READY_LATCH:-/tmp/trading-capture-gate.ready}"  # set once ready

ts() { date -u +"%Y-%m-%dT%H:%M:%SZ"; }
log() { echo "$(ts) $*"; }

compose() { docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE" "$@"; }

cd "$PROJECT_DIR" || { log "ERROR cannot cd to $PROJECT_DIR"; exit 1; }

running() {
  [ "$(docker inspect -f '{{.State.Running}}' "$1" 2>/dev/null || echo false)" = "true" ]
}

action_taken=0

# 1. containers up?
if ! running "$CAPTURE_CONTAINER" || ! running "$PG_CONTAINER"; then
  log "WARN a container is down (capture=$(running "$CAPTURE_CONTAINER" && echo up || echo down) pg=$(running "$PG_CONTAINER" && echo up || echo down)); running compose up -d"
  compose up -d >/dev/null 2>&1 && log "INFO compose up -d done" || log "ERROR compose up -d failed"
  action_taken=1
fi

# 2. data freshness (only meaningful once pg is up)
if running "$PG_CONTAINER"; then
  lag=$(docker exec "$PG_CONTAINER" psql -U trading -d trading_system -tAc \
    "SELECT COALESCE(round(extract(epoch FROM now()-max(event_time))), 999999) FROM order_books;" 2>/dev/null | tr -d '[:space:]')
  if [ -z "$lag" ]; then
    log "WARN could not read order_books freshness (pg query failed)"
  elif [ "$lag" -gt "$STALL_SECS" ]; then
    log "WARN order_books stalled: ${lag}s since last row (> ${STALL_SECS}s); restarting capture"
    compose restart "$CAPTURE_CONTAINER" >/dev/null 2>&1 \
      && log "INFO capture restarted" || log "ERROR capture restart failed"
    action_taken=1
  fi
fi

# 3. disk (alert only)
disk_pct=$(df --output=pcent / | tail -1 | tr -dc '0-9')
if [ -n "$disk_pct" ] && [ "$disk_pct" -gt "$DISK_WARN_PCT" ]; then
  log "WARN root filesystem ${disk_pct}% full (> ${DISK_WARN_PCT}%) — consider pruning order_books (manual decision)"
  action_taken=1
fi

# 4. imbalance gate (hourly, log-only). Count how many of the 18 feed×horizon clear
# the non-overlapping sample floor; same query as deploy/analysis/README.md.
gate_due() {
  [ ! -f "$GATE_STAMP" ] && return 0
  [ "$(( $(date +%s) - $(stat -c %Y "$GATE_STAMP" 2>/dev/null || echo 0) ))" -ge "$GATE_CHECK_SECS" ]
}
if running "$PG_CONTAINER" && [ ! -f "$GATE_READY_LATCH" ] && gate_due; then
  touch "$GATE_STAMP"
  gate=$(docker exec "$PG_CONTAINER" psql -U trading -d trading_system -tAc "
    WITH horizons(h) AS (VALUES (10),(30),(60)),
    ticks AS (SELECT exchange, symbol, date_trunc('second', event_time) AS ts
              FROM order_books WHERE bid_size + ask_size > 0 GROUP BY 1,2,3)
    SELECT count(*) FILTER (WHERE n >= $GATE_FLOOR) || '/' || count(*)
    FROM (SELECT count(*) FILTER (WHERE extract(epoch FROM t.ts)::bigint % h.h = 0) AS n
          FROM ticks t CROSS JOIN horizons h GROUP BY t.exchange, t.symbol, h.h) g;
    " 2>/dev/null | tr -d '[:space:]')
  if [ -z "$gate" ]; then
    log "WARN could not read imbalance gate (pg query failed)"
  elif [ "$gate" = "18/18" ]; then
    log "GATE ready ($gate cleared floor ${GATE_FLOOR}) — time to run deploy/analysis/*.sql (manual)"
    touch "$GATE_READY_LATCH"   # latch: stop re-checking once ready
    action_taken=1
  else
    log "INFO imbalance gate waiting: $gate feeds cleared floor ${GATE_FLOOR}"
  fi
fi

[ "$action_taken" -eq 0 ] && log "OK capture healthy (freshness ${lag:-?}s, disk ${disk_pct:-?}%)"
exit 0
