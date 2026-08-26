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
  rm -f "$XIAO_STOP" "$XIAO_RESTART" "$XIAO_WATCHDOG_PID"
  rotate_xiao_log "$XIAO_WATCHDOG_LOG"
  XIAO_LOG_TO_FILE=1 nohup "$XIAO_WATCHDOG" >/dev/null 2>&1 </dev/null &
  printf '%s\n' "$!" > "$XIAO_WATCHDOG_PID"
  chmod 0600 "$XIAO_WATCHDOG_PID" 2>/dev/null || true
  xiao_log 'Watchdog dimulai.'
}

stop_watchdog() {
  ensure_xiao_dirs || return 1
  touch "$XIAO_STOP"
  rm -f "$XIAO_RESTART"
  stop_owned_pid_file "$XIAO_WATCHDOG_PID" "$XIAO_WATCHDOG"
  stop_owned_pid_file "$XIAO_DAEMON_PID" "$XIAO_BINARY"
  xiao_log 'Watchdog dan xiao daemon dihentikan.'
}

wait_daemon_replaced() {
  previous_pid=${1:-}
  remaining=${2:-20}
  while [ "$remaining" -gt 0 ]; do
    current_pid=$(pid_from_file "$XIAO_DAEMON_PID" 2>/dev/null || true)
    if [ "$current_pid" != "$previous_pid" ] && pid_matches "$current_pid" "$XIAO_BINARY"; then
      xiao_log "xiao daemon siap (PID $current_pid)."
      return 0
    fi
    sleep 1
    remaining=$((remaining - 1))
  done
  xiao_log 'xiao daemon belum siap setelah 20 detik; periksa watchdog.log dan daemon.log.' >&2
  return 1
}

restart_daemon() {
  ensure_xiao_dirs || return 1
  previous_pid=$(pid_from_file "$XIAO_DAEMON_PID" 2>/dev/null || true)
  if ! watchdog_is_running; then
    xiao_log 'Watchdog tidak aktif; memulai lifecycle xiao.'
    start_watchdog || return 1
    wait_daemon_replaced "$previous_pid" 20
    return
  fi

  touch "$XIAO_RESTART" || return 1
  if pid_matches "$previous_pid" "$XIAO_BINARY"; then
    kill -TERM "$previous_pid" 2>/dev/null || {
      rm -f "$XIAO_RESTART"
      return 1
    }
    xiao_log "Restart xiao daemon diminta melalui watchdog (PID $previous_pid)."
  else
    xiao_log 'xiao daemon tidak aktif; watchdog diminta memulihkannya sekarang.'
  fi
  wait_daemon_replaced "$previous_pid" 20
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
    echo "✓ Daemon    : RUNNING (PID $(pid_from_file "$XIAO_DAEMON_PID"))"
  else
    echo '✗ Daemon    : STOPPED'
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
  restart) restart_daemon ;;
  status) show_status ;;
  status-json) show_status_json ;;
  snapshot) run_xiao_admin snapshot ;;
  apply-base64) run_encoded_admin_command apply-base64 "${2:-}" ;;
  fetch-models-base64) run_encoded_admin_command fetch-models-base64 "${2:-}" ;;
  manager-get-base64) run_encoded_admin_command manager-get-base64 "${2:-}" ;;
  manager-post-base64) run_encoded_admin_command manager-post-base64 "${2:-}" ;;
  logs) tail -n "${2:-120}" "$XIAO_DAEMON_LOG" 2>/dev/null ;;
  pair) echo 'Pairing manual tidak diperlukan; wrapper Termux dikelola module.' ;;
  wrappers) install_termux_wrappers ;;
  *) echo 'usage: action.sh [start|stop|restart|status|status-json|snapshot|apply-base64 PAYLOAD|fetch-models-base64 PAYLOAD|manager-get-base64 PAYLOAD|manager-post-base64 PAYLOAD|logs [N]|wrappers]'; exit 2 ;;
esac
