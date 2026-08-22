#!/system/bin/sh

MODDIR=${0%/*}
# shellcheck source=module/common.sh
. "$MODDIR/common.sh"
# shellcheck source=module/termux.sh
. "$MODDIR/termux.sh"

start_watchdog() {
  ensure_xiao_dirs || return 1
  watchdog_pid=$(pid_from_file "$XIAO_WATCHDOG_PID" 2>/dev/null || true)
  if pid_matches "$watchdog_pid" "$XIAO_WATCHDOG"; then
    xiao_log "Watchdog sudah aktif (PID $watchdog_pid)."
    return 0
  fi
  rm -f "$XIAO_STOP" "$XIAO_WATCHDOG_PID"
  rotate_xiao_log "$XIAO_WATCHDOG_LOG"
  XIAO_LOG_TO_FILE=1 nohup "$XIAO_WATCHDOG" >/dev/null 2>&1 </dev/null &
  printf '%s\n' "$!" > "$XIAO_WATCHDOG_PID"
  chmod 0600 "$XIAO_WATCHDOG_PID" 2>/dev/null || true
  xiao_log 'Watchdog dimulai.'
}

stop_watchdog() {
  ensure_xiao_dirs || return 1
  touch "$XIAO_STOP"
  stop_owned_pid_file "$XIAO_WATCHDOG_PID" "$XIAO_WATCHDOG"
  stop_owned_pid_file "$XIAO_DAEMON_PID" "$XIAOD_BINARY"
  xiao_log 'Watchdog dan xiaod dihentikan.'
}

show_status() {
  ensure_xiao_dirs || return 1
  wrapper_status=$(termux_wrappers_status)
  [ "$wrapper_status" = ready ] || install_termux_wrappers >/dev/null 2>&1 || true
  echo '===================================='
  echo '       xiao Diagnostic Panel'
  echo '===================================='
  echo "Date: $(date '+%Y-%m-%d %H:%M:%S')"
  echo
  if daemon_is_running; then
    echo "✓ xiaod     : RUNNING (PID $(pid_from_file "$XIAO_DAEMON_PID"))"
  else
    echo '✗ xiaod     : STOPPED'
  fi
  if watchdog_is_running; then
    echo "✓ Watchdog  : ACTIVE (PID $(pid_from_file "$XIAO_WATCHDOG_PID"))"
  else
    echo '✗ Watchdog  : STOPPED'
  fi
  echo "✓ Termux CLI: $(termux_wrappers_status)"
  echo "✓ Config    : $XIAO_CONFIG"
  echo "✓ Data      : $XIAO_DATA_DIR"
  if [ -f "$MODDIR/disable" ] || [ -f "$XIAO_DISABLE" ]; then
    echo '⚠ Autostart : DISABLED'
  else
    echo '✓ Autostart : ENABLED'
  fi
  echo
  echo '[RECENT WATCHDOG LOG]'
  tail -n 8 "$XIAO_WATCHDOG_LOG" 2>/dev/null || true
  echo '===================================='
}

show_status_json() {
  ensure_xiao_dirs || return 1
  json_daemon_running=false
  json_daemon_pid=null
  if daemon_is_running; then
    json_daemon_running=true
    json_daemon_pid=$(pid_from_file "$XIAO_DAEMON_PID")
  fi
  json_watchdog_running=false
  json_watchdog_pid=null
  if watchdog_is_running; then
    json_watchdog_running=true
    json_watchdog_pid=$(pid_from_file "$XIAO_WATCHDOG_PID")
  fi
  json_autostart=true
  if [ -f "$MODDIR/disable" ] || [ -f "$XIAO_DISABLE" ]; then
    json_autostart=false
  fi
  printf '{"daemon":{"running":%s,"pid":%s},"watchdog":{"running":%s,"pid":%s},"autostart":%s}\n' \
    "$json_daemon_running" "$json_daemon_pid" \
    "$json_watchdog_running" "$json_watchdog_pid" "$json_autostart"
}

run_encoded_admin_command() {
  [ "$#" -eq 2 ] || {
    echo "usage: action.sh $1 PAYLOAD" >&2
    return 2
  }
  case "$2" in
    ''|*[!A-Za-z0-9_-]*)
      echo 'invalid base64url admin payload' >&2
      return 2
      ;;
  esac
  run_xiao_admin "$1" "$2"
}

case "${1:-status}" in
  start) start_watchdog ;;
  stop) stop_watchdog ;;
  restart)
    stop_watchdog
    rm -f "$XIAO_STOP"
    start_watchdog
    ;;
  status) show_status ;;
  status-json) show_status_json ;;
  snapshot) run_xiao_admin snapshot ;;
  apply-base64) run_encoded_admin_command apply-base64 "${2:-}" ;;
  fetch-models-base64) run_encoded_admin_command fetch-models-base64 "${2:-}" ;;
  logs) tail -n "${2:-120}" "$XIAO_DAEMON_LOG" 2>/dev/null ;;
  pair) echo 'Pairing manual tidak diperlukan; wrapper Termux dikelola module.' ;;
  wrappers) install_termux_wrappers ;;
  *) echo 'usage: action.sh [start|stop|restart|status|status-json|snapshot|apply-base64 PAYLOAD|fetch-models-base64 PAYLOAD|logs [N]|wrappers]'; exit 2 ;;
esac
