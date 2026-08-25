#!/usr/bin/env bash
set -Eeuo pipefail

[ "${GITHUB_ACTIONS:-}" = true ] || {
  echo 'Module packaging is GitHub-Actions-only; trigger the ci workflow.' >&2
  exit 1
}

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUST_TARGET=${XIAO_TARGET:-aarch64-linux-android}
PROFILE=${PROFILE:-release}
OUT="$ROOT/dist"
MODULE_VERSION=${VERSION:-$(awk -F'"' '/^version[[:space:]]*=/ { print $2; exit }' "$ROOT/Cargo.toml")}
VERSION_CODE=${VERSION_CODE:-$(awk -F. '{ print ($1 * 10000) + ($2 * 100) + $3 }' <<< "$MODULE_VERSION")}
ARCHIVE="$OUT/xiao-v${MODULE_VERSION}-kernelsu-arm64.zip"

if [ "$RUST_TARGET" = native ]; then
  BUILD="$ROOT/target/$PROFILE"
else
  BUILD="$ROOT/target/$RUST_TARGET/$PROFILE"
fi

[ -x "$BUILD/xiao" ] || { echo "Missing executable: $BUILD/xiao" >&2; exit 1; }
file "$BUILD/xiao" | grep -q 'ELF 64-bit.*ARM aarch64' || {
  echo "Not an Android arm64 ELF binary: $BUILD/xiao" >&2
  exit 1
}

mkdir -p "$OUT"
STAGE=$(mktemp -d "${TMPDIR:-$OUT}/xiao-kernelsu-arm64.XXXXXX")
cleanup() { rm -rf -- "$STAGE"; }
trap cleanup EXIT

cp -a "$ROOT/module/." "$STAGE/"
mkdir -p "$STAGE/bin"
rm -f "$STAGE/bin/.gitkeep"
sed -i \
  -e "s/@MODULE_VERSION@/$MODULE_VERSION/g" \
  -e "s/@VERSION_CODE@/$VERSION_CODE/g" \
  "$STAGE/module.prop"
install -m 0755 "$BUILD/xiao" "$STAGE/bin/xiao"

find "$STAGE" -type d -exec chmod 0755 {} +
find "$STAGE" -type f -exec chmod 0644 {} +
find "$STAGE" -type f -name '*.sh' -exec chmod 0755 {} +
chmod 0755 "$STAGE/bin/xiao"
chmod 0755 "$STAGE/termux/xiao-wrapper"

required=(
  module.prop
  customize.sh
  common.sh
  termux.sh
  post-fs-data.sh
  service.sh
  watchdog.sh
  action.sh
  uninstall.sh
  skip_mount
  config.example.toml
  bin/xiao
  webroot/index.html
  webroot/assets/app.js
  webroot/assets/app.css
  webroot/assets/ksu-bridge.js
  termux/xiao-wrapper
)
for entry in "${required[@]}"; do
  [ -e "$STAGE/$entry" ] || {
    echo "Missing required module entry: $entry" >&2
    exit 1
  }
done
[ "$(find "$STAGE/bin" -maxdepth 1 -type f | wc -l)" -eq 1 ] || {
  echo 'Module must ship exactly one regular native executable.' >&2
  exit 1
}
[ ! -e "$STAGE/bin/xiaod" ] || {
  echo 'A second xiaod executable is forbidden in v0.3.' >&2
  exit 1
}

for script in "$STAGE"/*.sh; do
  sh -n "$script"
  head -n 1 "$script" | grep -Fxq '#!/system/bin/sh' || {
    echo "Module script must use /system/bin/sh: ${script##*/}" >&2
    exit 1
  }
done
sh -n "$STAGE/termux/xiao-wrapper"
head -n 1 "$STAGE/termux/xiao-wrapper" | grep -Fxq '#!/system/bin/sh'

grep -Fxq 'id=xiao' "$STAGE/module.prop"
grep -Fxq "version=v$MODULE_VERSION" "$STAGE/module.prop"
grep -Fxq "versionCode=$VERSION_CODE" "$STAGE/module.prop"
if grep -Fq '@MODULE_VERSION@' "$STAGE/module.prop"; then
  echo 'Unresolved module version placeholder' >&2
  exit 1
fi
if grep -Fq '@VERSION_CODE@' "$STAGE/module.prop"; then
  echo 'Unresolved module version-code placeholder' >&2
  exit 1
fi
grep -Fq '/data/adb/xiao' "$STAGE/config.example.toml"
cmp "$ROOT/module/webroot/index.html" "$STAGE/webroot/index.html"
cmp "$ROOT/module/webroot/assets/app.js" "$STAGE/webroot/assets/app.js"
cmp "$ROOT/module/webroot/assets/app.css" "$STAGE/webroot/assets/app.css"
cmp "$ROOT/module/webroot/assets/ksu-bridge.js" "$STAGE/webroot/assets/ksu-bridge.js"

if find "$STAGE" -type f \( -name '*.db' -o -name '*.db-*' -o -name '*.secret' \
  -o -name '*.log' -o -name '*.pid' -o -name 'client.toml' \) | grep -q .; then
  echo 'Runtime database, credential, log, PID, or client config found in module stage' >&2
  exit 1
fi

if command -v readelf >/dev/null 2>&1; then
  readelf -l "$STAGE/bin/xiao" | grep '/system/bin/linker64' >/dev/null || {
    echo 'Unexpected Android interpreter: xiao' >&2
    exit 1
  }
  ! readelf -d "$STAGE/bin/xiao" | grep -E 'libssl|libcrypto|libstdc\+\+|libgcc_s' >/dev/null || {
    echo 'Unexpected non-system runtime dependency: xiao' >&2
    exit 1
  }
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
