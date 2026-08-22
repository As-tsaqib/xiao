#!/system/bin/sh

ui_print '***************************************'
ui_print '              xiao v0.1.0             '
ui_print '***************************************'

case "$ARCH" in
  arm64) ui_print '- ABI: arm64 (supported)' ;;
  *) abort "! Unsupported ABI: $ARCH; xiao v0.1.0 requires Android arm64." ;;
esac

# ZIP extraction does not have to preserve executable bits. Validate regular
# files first, then apply authoritative module permissions before executing.
[ -f "$MODPATH/bin/xiaod" ] || abort '! Incomplete ZIP: bin/xiaod is missing.'
[ -f "$MODPATH/bin/xiao" ] || abort '! Incomplete ZIP: bin/xiao is missing.'
[ -f "$MODPATH/webroot/index.html" ] || abort '! Incomplete ZIP: WebUI is missing.'
[ -f "$MODPATH/config.example.toml" ] || abort '! Incomplete ZIP: config example is missing.'

for script in service.sh supervisor.sh action.sh uninstall.sh; do
  [ -f "$MODPATH/$script" ] || abort "! Incomplete ZIP: $script is missing."
  set_perm "$MODPATH/$script" 0 0 0755
done
set_perm "$MODPATH/bin/xiaod" 0 0 0755
set_perm "$MODPATH/bin/xiao" 0 0 0755
set_perm "$MODPATH/config.example.toml" 0 0 0644

# KernelSU Manager applies the WebUI permissions and SELinux context after
# extraction; leave webroot under that lifecycle instead of overriding it.

# Catch a wrong ABI/linker build during installation instead of failing later
# in late_start with only a supervisor loop in the logs.
"$MODPATH/bin/xiaod" --version >/dev/null 2>&1 || abort '! xiaod cannot execute on this device.'
"$MODPATH/bin/xiao" --version >/dev/null 2>&1 || abort '! xiao cannot execute on this device.'

umask 077
mkdir -p /data/adb/xiao/data /data/adb/xiao/secrets /data/adb/xiao/logs \
  /data/adb/xiao/cache /data/adb/xiao/ipc
chmod 0700 /data/adb/xiao /data/adb/xiao/data /data/adb/xiao/secrets \
  /data/adb/xiao/logs /data/adb/xiao/cache /data/adb/xiao/ipc 2>/dev/null || true
if [ ! -f /data/adb/xiao/config.toml ]; then
  cp "$MODPATH/config.example.toml" /data/adb/xiao/config.toml || abort '! Cannot create persistent config.'
fi
chmod 0600 /data/adb/xiao/config.toml 2>/dev/null || true

ui_print '- Persistent state: /data/adb/xiao'
ui_print '- Open the module WebUI after reboot.'
