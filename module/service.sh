#!/system/bin/sh

MODDIR=${0%/*}
DATA=/data/adb/xiao
umask 077
mkdir -p "$DATA/data" "$DATA/secrets" "$DATA/logs" "$DATA/cache" "$DATA/ipc"
chmod 0700 "$DATA" "$DATA/data" "$DATA/secrets" "$DATA/logs" "$DATA/cache" "$DATA/ipc" 2>/dev/null || true
[ -f "$DATA/config.toml" ] || cp "$MODDIR/config.example.toml" "$DATA/config.toml"
chmod 0600 "$DATA/config.toml" 2>/dev/null || true

n=0
while [ "$(getprop sys.boot_completed 2>/dev/null)" != 1 ] && [ "$n" -lt 30 ]; do
  sleep 2
  n=$((n + 1))
done
sleep 2

exec "$MODDIR/supervisor.sh" >>"$DATA/logs/supervisor.log" 2>&1
