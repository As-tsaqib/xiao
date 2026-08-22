#!/system/bin/sh

MODDIR=${0%/*}
# shellcheck source=module/common.sh
. "$MODDIR/common.sh"
# shellcheck source=module/termux.sh
. "$MODDIR/termux.sh"

touch "$XIAO_STOP" 2>/dev/null || true
rm -f "$XIAO_RESTART"
stop_owned_pid_file "$XIAO_IPC_DIR/supervisor.pid" "$MODDIR/supervisor.sh"
stop_owned_pid_file "$XIAO_DATA_DIR/xiaod.pid" "$XIAOD_BINARY"
stop_owned_pid_file "$XIAO_WATCHDOG_PID" "$XIAO_WATCHDOG"
stop_owned_pid_file "$XIAO_DAEMON_PID" "$XIAOD_BINARY"
remove_termux_wrappers || true
rm -f "$XIAO_STOP" "$XIAO_RESTART"
xiao_log 'Data /data/adb/xiao dipertahankan agar config dan credential tidak hilang.'
