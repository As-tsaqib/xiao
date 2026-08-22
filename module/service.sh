#!/system/bin/sh

MODDIR=${0%/*}
# shellcheck source=module/common.sh
. "$MODDIR/common.sh"
# shellcheck source=module/termux.sh
. "$MODDIR/termux.sh"

ensure_xiao_dirs || exit 1
install_termux_wrappers >> "$XIAO_WATCHDOG_LOG" 2>&1 || true

[ -f "$MODDIR/disable" ] && exit 0
[ -f "$XIAO_DISABLE" ] && exit 0
[ -x "$XIAO_WATCHDOG" ] || exit 1

watchdog_pid=$(pid_from_file "$XIAO_WATCHDOG_PID" 2>/dev/null || true)
if pid_matches "$watchdog_pid" "$XIAO_WATCHDOG"; then
  exit 0
fi
rm -f "$XIAO_WATCHDOG_PID" "$XIAO_STOP"

boot_wait=0
while [ "$(getprop sys.boot_completed 2>/dev/null)" != 1 ] && [ "$boot_wait" -lt 30 ]; do
  sleep 2
  boot_wait=$((boot_wait + 1))
done
sleep 2

rotate_xiao_log "$XIAO_WATCHDOG_LOG"
XIAO_LOG_TO_FILE=1 nohup "$XIAO_WATCHDOG" >/dev/null 2>&1 </dev/null &
printf '%s\n' "$!" > "$XIAO_WATCHDOG_PID"
chmod 0600 "$XIAO_WATCHDOG_PID" 2>/dev/null || true
