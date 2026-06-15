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

[ "$action_taken" -eq 0 ] && log "OK capture healthy (freshness ${lag:-?}s, disk ${disk_pct:-?}%)"
exit 0
