#!/system/bin/sh

MODDIR=${0%/*}
DATA=/data/adb/xiao
PIDFILE="$DATA/xiaod.pid"
SUPERVISOR_PID="$DATA/ipc/supervisor.pid"
SUPERVISOR_LOG="$DATA/logs/supervisor.log"

valid_pid() {
  case "${1:-}" in
    ''|*[!0-9]*) return 1 ;;
  esac
  [ "$1" -gt 1 ] 2>/dev/null
}

pid_matches() {
  check_pid=$1
  check_text=$2
  valid_pid "$check_pid" || return 1
  kill -0 "$check_pid" 2>/dev/null || return 1
  tr '\000' ' ' < "/proc/$check_pid/cmdline" 2>/dev/null | grep -Fq "$check_text"
}

stop_supervisor() {
  supervisor=$(cat "$SUPERVISOR_PID" 2>/dev/null || true)
  if pid_matches "$supervisor" "$MODDIR/supervisor.sh"; then
    kill -TERM "$supervisor" 2>/dev/null || true
  fi
  i=0
  while pid_matches "$supervisor" "$MODDIR/supervisor.sh" && [ "$i" -lt 20 ]; do
    sleep 1
    i=$((i + 1))
  done
  if pid_matches "$supervisor" "$MODDIR/supervisor.sh"; then
    kill -KILL "$supervisor" 2>/dev/null || true
  fi
  rm -f "$SUPERVISOR_PID"
}

stop_daemon() {
  daemon=$(cat "$PIDFILE" 2>/dev/null || true)
  if pid_matches "$daemon" "$MODDIR/bin/xiaod"; then
    kill -TERM "$daemon" 2>/dev/null || true
    i=0
    while pid_matches "$daemon" "$MODDIR/bin/xiaod" && [ "$i" -lt 15 ]; do
      sleep 1
      i=$((i + 1))
    done
    if pid_matches "$daemon" "$MODDIR/bin/xiaod"; then
      kill -KILL "$daemon" 2>/dev/null || true
    fi
  fi
  rm -f "$PIDFILE"
}

start_supervisor() {
  mkdir -p "$DATA/logs" "$DATA/ipc"
  supervisor=$(cat "$SUPERVISOR_PID" 2>/dev/null || true)
  if pid_matches "$supervisor" "$MODDIR/supervisor.sh"; then
    return 0
  fi
  rm -f "$SUPERVISOR_PID"
  nohup "$MODDIR/supervisor.sh" >>"$SUPERVISOR_LOG" 2>&1 </dev/null &
}

case "${1:-restart}" in
  restart)
    stop_supervisor
    stop_daemon
    start_supervisor
    echo 'xiao daemon and supervisor restart requested.'
    ;;
  start)
    start_supervisor
    echo 'xiao supervisor start requested.'
    ;;
  stop)
    stop_supervisor
    stop_daemon
    echo 'xiao daemon and supervisor stopped.'
    ;;
  status)
    XIAO_CONFIG="$DATA/config.toml" "$MODDIR/bin/xiao" admin snapshot
    ;;
  logs)
    XIAO_CONFIG="$DATA/config.toml" "$MODDIR/bin/xiao" admin logs "${2:-120}"
    ;;
  pair)
    echo 'Warning: pairing output contains the limited client credential.' >&2
    XIAO_CONFIG="$DATA/config.toml" "$MODDIR/bin/xiao" admin client-config
    ;;
  *) echo "usage: action.sh [start|stop|restart|status|logs [N]|pair]"; exit 2;;
esac
