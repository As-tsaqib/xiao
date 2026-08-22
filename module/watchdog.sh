#!/system/bin/sh

MODDIR=${0%/*}
# shellcheck source=module/common.sh
. "$MODDIR/common.sh"

INTERVAL=5
STABLE_TIME=120
MAX_BACKOFF=60
child=
backoff=0

cleanup() {
  trap - INT TERM EXIT
  if [ -n "$child" ] && pid_matches "$child" "$XIAOD_BINARY"; then
    kill -TERM "$child" 2>/dev/null || true
    wait_owned_exit "$child" "$XIAOD_BINARY" 15 || kill -KILL "$child" 2>/dev/null || true
  fi
  rm -f "$XIAO_DAEMON_PID" "$XIAO_WATCHDOG_PID"
  exit 0
}
trap 'cleanup' INT TERM EXIT

ensure_xiao_dirs || exit 1

while true; do
  if [ -f "$MODDIR/disable" ] || [ -f "$XIAO_DISABLE" ] || [ -f "$XIAO_STOP" ]; then
    xiao_log 'Watchdog dihentikan oleh marker disable/stop.'
    exit 0
  fi

  if [ -f "$XIAO_RESTART" ]; then
    rm -f "$XIAO_RESTART"
    backoff=0
    xiao_log 'Permintaan restart daemon diterapkan.'
  fi

  rotate_xiao_log "$XIAO_DAEMON_LOG"
  started=$(date +%s 2>/dev/null || printf '0')
  case "$started" in ''|*[!0-9]*) started=0 ;; esac

  HOME="$XIAO_DATA_DIR" XIAO_HOME="$XIAO_DATA_DIR" \
    XIAO_CONFIG="$XIAO_CONFIG" XIAO_CLIENT_CONFIG="$XIAO_CLIENT_CONFIG" \
    TMPDIR="$XIAO_TMP_DIR" XIAO_BOOT_START=1 \
    "$XIAOD_BINARY" >> "$XIAO_DAEMON_LOG" 2>&1 &
  child=$!
  printf '%s\n' "$child" > "$XIAO_DAEMON_PID"
  chmod 0600 "$XIAO_DAEMON_PID" 2>/dev/null || true
  xiao_log "xiaod dimulai (PID $child)."

  provision_wait=0
  while [ "$provision_wait" -lt 30 ] && pid_matches "$child" "$XIAOD_BINARY"; do
    if ensure_client_config; then
      break
    fi
    sleep 1
    provision_wait=$((provision_wait + 1))
  done

  wait "$child"
  exit_code=$?
  child=
  rm -f "$XIAO_DAEMON_PID"

  if [ -f "$XIAO_RESTART" ]; then
    rm -f "$XIAO_RESTART"
    backoff=0
    xiao_log "xiaod keluar dengan kode $exit_code karena restart diminta; mulai ulang sekarang."
    continue
  fi

  if ! auto_restart_enabled; then
    xiao_log "xiaod keluar dengan kode $exit_code; auto_restart=false."
    exit 0
  fi

  now=$(date +%s 2>/dev/null || printf '0')
  case "$now" in ''|*[!0-9]*) now=0 ;; esac
  runtime=0
  [ "$started" -gt 0 ] && [ "$now" -ge "$started" ] && runtime=$((now - started))
  if [ "$runtime" -ge "$STABLE_TIME" ]; then
    backoff=0
  elif [ "$backoff" -eq 0 ]; then
    backoff=$INTERVAL
  else
    backoff=$((backoff * 2))
    [ "$backoff" -gt "$MAX_BACKOFF" ] && backoff=$MAX_BACKOFF
  fi
  delay=$backoff
  [ "$delay" -gt 0 ] || delay=$INTERVAL
  xiao_log "xiaod keluar dengan kode $exit_code setelah ${runtime}s; mulai ulang dalam ${delay}s."
  slept=0
  while [ "$slept" -lt "$delay" ] && [ ! -f "$XIAO_RESTART" ]; do
    [ -f "$MODDIR/disable" ] || [ -f "$XIAO_DISABLE" ] || [ -f "$XIAO_STOP" ] || {
      sleep 1
      slept=$((slept + 1))
      continue
    }
    break
  done
done
