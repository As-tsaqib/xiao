#!/system/bin/sh

MODDIR=${0%/*}
DATA=/data/adb/xiao
PIDFILE="$DATA/xiaod.pid"
LOCK="$DATA/ipc/supervisor.pid"
LOG="$DATA/logs/daemon.log"
CONFIG="$DATA/config.toml"
child=
valid_pid() {
  case "${1:-}" in
    ''|*[!0-9]*) return 1 ;;
  esac
  [ "$1" -gt 1 ] 2>/dev/null
}
auto_restart_enabled() {
  # Parse only the small [gateway] boolean needed by the supervisor. Missing/invalid
  # values keep the safe historical default (enabled).
  [ ! -f "$CONFIG" ] && return 0
  value=$(awk '
    /^\[gateway\][[:space:]]*$/ { in_gateway=1; next }
    /^\[/ { in_gateway=0 }
    in_gateway && /^[[:space:]]*auto_restart[[:space:]]*=/ {
      sub(/^[^=]*=[[:space:]]*/, ""); sub(/[[:space:]#].*$/, ""); print; exit
    }
  ' "$CONFIG" 2>/dev/null)
  [ "$value" != "false" ]
}
cleanup() {
  if [ -n "$child" ] && kill -0 "$child" 2>/dev/null; then
    kill -TERM "$child" 2>/dev/null || true
    wait "$child" 2>/dev/null || true
  fi
  rm -f "$PIDFILE" "$LOCK"
}
trap 'cleanup; exit 0' INT TERM
trap 'rm -f "$LOCK"' EXIT
if [ -f "$LOCK" ]; then
  old=$(cat "$LOCK" 2>/dev/null || true)
  if valid_pid "$old" && kill -0 "$old" 2>/dev/null; then
    exit 0
  fi
  rm -f "$LOCK"
fi
echo $$ > "$LOCK"
chmod 600 "$LOCK" 2>/dev/null || true
backoff=2
while true; do
  if [ -f "$MODDIR/disable" ]; then
    if [ -n "$child" ] && kill -0 "$child" 2>/dev/null; then
      kill -TERM "$child" 2>/dev/null || true
      wait "$child" 2>/dev/null || true
      child=
      rm -f "$PIDFILE"
    fi
    sleep 10
    continue
  fi
  # Keep log growth bounded on long-lived phones without requiring logrotate.
  if [ -f "$LOG" ]; then
    size=$(wc -c < "$LOG" 2>/dev/null || echo 0)
    if [ "$size" -gt 2097152 ]; then tail -c 1048576 "$LOG" > "$LOG.tmp" 2>/dev/null && mv "$LOG.tmp" "$LOG"; fi
  fi
  XIAO_BOOT_START=1 XIAO_CONFIG="$CONFIG" "$MODDIR/bin/xiaod" >>"$LOG" 2>&1 &
  child=$!
  echo "$child" > "$PIDFILE"
  chmod 600 "$PIDFILE" 2>/dev/null || true
  started=$(date +%s)
  wait "$child"; code=$?
  child=
  rm -f "$PIDFILE"
  if ! auto_restart_enabled; then
    echo "$(date -Iseconds 2>/dev/null || date) daemon exited code=$code; auto_restart=false, supervisor stopping" >>"$LOG"
    exit 0
  fi
  now=$(date +%s); runtime=$((now-started)); delay=$backoff
  if [ "$runtime" -ge 120 ]; then backoff=2; delay=2; else backoff=$((backoff*2)); [ "$backoff" -gt 60 ] && backoff=60; fi
  echo "$(date -Iseconds 2>/dev/null || date) daemon exited code=$code; restart in ${delay}s" >>"$LOG"
  sleep "$delay"
done
