#!/system/bin/sh

# Stop module-owned processes, but preserve /data/adb/xiao so update/reinstall
# cannot destroy config, sessions, accounts, or secrets.
MODDIR=${0%/*}
DATA=/data/adb/xiao
PIDFILE="$DATA/xiaod.pid"
SUPERVISOR_PID="$DATA/ipc/supervisor.pid"

valid_pid() {
  case "${1:-}" in
    ''|*[!0-9]*) return 1 ;;
  esac
  [ "$1" -gt 1 ] 2>/dev/null
}

stop_if_owned() {
  pid=$1
  marker=$2
  valid_pid "$pid" || return 0
  kill -0 "$pid" 2>/dev/null || return 0
  tr '\000' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -Fq "$marker" || return 0
  kill -TERM "$pid" 2>/dev/null || true
}

supervisor=$(cat "$SUPERVISOR_PID" 2>/dev/null || true)
stop_if_owned "$supervisor" "$MODDIR/supervisor.sh"
daemon=$(cat "$PIDFILE" 2>/dev/null || true)
stop_if_owned "$daemon" "$MODDIR/bin/xiaod"
rm -f "$PIDFILE" "$SUPERVISOR_PID"
