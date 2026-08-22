#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUST_TARGET=${XIAO_TARGET:-aarch64-linux-android}
PROFILE=${PROFILE:-release}
OUT="$ROOT/dist"
mkdir -p "$OUT"
STAGE=$(mktemp -d "$OUT/.xiao-termux-arm64.XXXXXX")
trap 'rm -rf "$STAGE"' EXIT
if [ "$RUST_TARGET" = native ]; then
  BUILD="$ROOT/target/$PROFILE"
else
  BUILD="$ROOT/target/$RUST_TARGET/$PROFILE"
fi
[ -x "$BUILD/xiao" ] || { echo "Missing executable: $BUILD/xiao" >&2; exit 1; }
[ -x "$BUILD/xiaod" ] || { echo "Missing executable: $BUILD/xiaod" >&2; exit 1; }
for binary in "$BUILD/xiao" "$BUILD/xiaod"; do
  file "$binary" | grep -q 'ELF 64-bit.*ARM aarch64' || {
    echo "Not an Android arm64 ELF binary: $binary" >&2
    exit 1
  }
done
cp "$BUILD/xiao" "$STAGE/xiao"
cp "$BUILD/xiaod" "$STAGE/xiaod"
cp "$ROOT/termux/install-client.sh" "$STAGE/install-client.sh"
cp "$ROOT/docs/TERMUX.md" "$STAGE/README.md"
cp "$ROOT/docs/BINARY_TEST.md" "$STAGE/TESTING.md"
cp "$ROOT/config/config.termux-test.toml" "$STAGE/config.termux-test.toml"
chmod 0755 "$STAGE/xiao" "$STAGE/xiaod" "$STAGE/install-client.sh"
chmod 0644 "$STAGE/README.md" "$STAGE/TESTING.md" "$STAGE/config.termux-test.toml"
(cd "$STAGE" && sha256sum xiao xiaod > SHA256SUMS)
chmod 0644 "$STAGE/SHA256SUMS"
if command -v readelf >/dev/null 2>&1; then
  for binary in "$STAGE/xiao" "$STAGE/xiaod"; do
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
touch -t 202601010000 "$STAGE/xiao" "$STAGE/xiaod" "$STAGE/install-client.sh" \
  "$STAGE/README.md" "$STAGE/TESTING.md" "$STAGE/config.termux-test.toml" \
  "$STAGE/SHA256SUMS"
ARCHIVE="$OUT/xiao-v0.1.0-termux-arm64.zip"
rm -f "$ARCHIVE" "$ARCHIVE.sha256"
(cd "$STAGE" && zip -X -q "$OUT/xiao-v0.1.0-termux-arm64.zip" \
  xiao xiaod install-client.sh README.md TESTING.md config.termux-test.toml SHA256SUMS)
unzip -t "$ARCHIVE" >/dev/null
(
  cd "$OUT"
  sha256sum "${ARCHIVE##*/}" > "${ARCHIVE##*/}.sha256"
)
echo "$ARCHIVE"
