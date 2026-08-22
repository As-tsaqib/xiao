#!/system/bin/sh

ui_print '***************************************'
ui_print '              xiao Module              '
ui_print '***************************************'

case "$ARCH" in
  arm64) ui_print '- ABI: arm64 (supported)' ;;
  *) abort "! Unsupported ABI: $ARCH; xiao requires Android arm64." ;;
esac

for required in bin/xiaod bin/xiao config.example.toml webroot/index.html \
  common.sh termux.sh post-fs-data.sh service.sh watchdog.sh action.sh \
  uninstall.sh termux/xiao-wrapper; do
  [ -f "$MODPATH/$required" ] || abort "! Incomplete ZIP: $required is missing."
done

set_perm "$MODPATH/bin/xiaod" 0 0 0755
set_perm "$MODPATH/bin/xiao" 0 0 0755
for module_script in common.sh termux.sh post-fs-data.sh service.sh watchdog.sh \
  action.sh uninstall.sh termux/xiao-wrapper; do
  set_perm "$MODPATH/$module_script" 0 0 0755
done
set_perm "$MODPATH/config.example.toml" 0 0 0644

"$MODPATH/bin/xiaod" --version >/dev/null 2>&1 || abort '! xiaod cannot execute on this device.'
"$MODPATH/bin/xiao" --version >/dev/null 2>&1 || abort '! xiao cannot execute on this device.'

MODDIR=$MODPATH
# shellcheck source=module/common.sh
. "$MODPATH/common.sh"
# shellcheck source=module/termux.sh
. "$MODPATH/termux.sh"
ensure_xiao_dirs || abort '! Cannot initialize /data/adb/xiao.'

ui_print '- Installing managed Termux wrappers...'
if ! install_termux_wrappers; then
  ui_print '! Termux not found; service/Action will retry automatically.'
fi
ui_print '- Persistent state: /data/adb/xiao'
ui_print '- Reboot to start xiaod under watchdog supervision.'
