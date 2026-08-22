#!/system/bin/sh

MODDIR=${0%/*}
# shellcheck source=module/common.sh
. "$MODDIR/common.sh"

ensure_xiao_dirs || exit 1
# Android can reuse PIDs after reboot, so runtime ownership files are never
# trusted across boots.
rm -f "$XIAO_DAEMON_PID" "$XIAO_WATCHDOG_PID" "$XIAO_STOP" "$XIAO_RESTART" \
  "$XIAO_DATA_DIR/xiaod.pid" "$XIAO_IPC_DIR/supervisor.pid"
