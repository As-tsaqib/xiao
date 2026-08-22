#!/usr/bin/env bash
set -Eeuo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUST_TARGET=${XIAO_TARGET:-aarch64-linux-android}
PROFILE=${PROFILE:-release}
OUT="$ROOT/dist"
ARCHIVE="$OUT/xiao-v0.1.0-kernelsu-arm64.zip"

if [ "$RUST_TARGET" = native ]; then
  BUILD="$ROOT/target/$PROFILE"
else
  BUILD="$ROOT/target/$RUST_TARGET/$PROFILE"
fi

for binary in xiao xiaod; do
  [ -x "$BUILD/$binary" ] || {
    echo "Missing executable: $BUILD/$binary" >&2
    exit 1
  }
  file "$BUILD/$binary" | grep -q 'ELF 64-bit.*ARM aarch64' || {
    echo "Not an Android arm64 ELF binary: $BUILD/$binary" >&2
    exit 1
  }
done

mkdir -p "$OUT"
STAGE=$(mktemp -d "${TMPDIR:-$OUT}/xiao-kernelsu-arm64.XXXXXX")
cleanup() { rm -rf -- "$STAGE"; }
trap cleanup EXIT

cp -a "$ROOT/module/." "$STAGE/"
rm -rf -- "$STAGE/webroot"
cp -a "$ROOT/webui" "$STAGE/webroot"
cp "$ROOT/config/config.example.toml" "$STAGE/config.example.toml"
mkdir -p "$STAGE/bin"
install -m 0755 "$BUILD/xiaod" "$STAGE/bin/xiaod"
install -m 0755 "$BUILD/xiao" "$STAGE/bin/xiao"

find "$STAGE" -type d -exec chmod 0755 {} +
find "$STAGE" -type f -exec chmod 0644 {} +
find "$STAGE" -type f -name '*.sh' -exec chmod 0755 {} +
chmod 0755 "$STAGE/bin/xiao" "$STAGE/bin/xiaod"

required=(
  module.prop
  customize.sh
  service.sh
  supervisor.sh
  action.sh
  uninstall.sh
  skip_mount
  config.example.toml
  bin/xiao
  bin/xiaod
  webroot/index.html
  webroot/assets/app.js
  webroot/assets/app.css
  webroot/assets/ksu-bridge.js
)
for entry in "${required[@]}"; do
  [ -e "$STAGE/$entry" ] || {
    echo "Missing required module entry: $entry" >&2
    exit 1
  }
done

for script in "$STAGE"/*.sh; do
  sh -n "$script"
  head -n 1 "$script" | grep -Fxq '#!/system/bin/sh' || {
    echo "Module script must use /system/bin/sh: ${script##*/}" >&2
    exit 1
  }
done

grep -Fxq 'id=xiao' "$STAGE/module.prop"
grep -Fq '/data/adb/xiao' "$STAGE/config.example.toml"
cmp "$ROOT/webui/index.html" "$STAGE/webroot/index.html"
cmp "$ROOT/webui/assets/app.js" "$STAGE/webroot/assets/app.js"
cmp "$ROOT/webui/assets/app.css" "$STAGE/webroot/assets/app.css"
cmp "$ROOT/webui/assets/ksu-bridge.js" "$STAGE/webroot/assets/ksu-bridge.js"

if find "$STAGE" -type f \( -name '*.db' -o -name '*.db-*' -o -name '*.secret' \
  -o -name '*.log' -o -name '*.pid' -o -name 'client.toml' \) | grep -q .; then
  echo 'Runtime database, credential, log, PID, or client config found in module stage' >&2
  exit 1
fi

if command -v readelf >/dev/null 2>&1; then
  for binary in "$STAGE/bin/xiao" "$STAGE/bin/xiaod"; do
    readelf -l "$binary" | grep '/system/bin/linker64' >/dev/null || {
      echo "Unexpected Android interpreter: $binary" >&2
      exit 1
    }
    ! readelf -d "$binary" | grep -E 'libssl|libcrypto|libstdc\+\+|libgcc_s' >/dev/null || {
      echo "Unexpected non-system runtime dependency: $binary" >&2
      exit 1
    }
  done
fi

find "$STAGE" -exec touch -t 202601010000 {} +
rm -f -- "$ARCHIVE" "$ARCHIVE.sha256"
(
  cd "$STAGE"
  find . -mindepth 1 -print | LC_ALL=C sort | zip -X -q "$ARCHIVE" -@
)
unzip -t "$ARCHIVE" >/dev/null
(
  cd "$OUT"
  sha256sum "${ARCHIVE##*/}" > "${ARCHIVE##*/}.sha256"
)

echo "$ARCHIVE"
