#!/data/data/com.termux/files/usr/bin/sh
set -eu

PREFIX=${PREFIX:-/data/data/com.termux/files/usr}
MODE=${1:-standalone}
mkdir -p "$PREFIX/bin"

case "$MODE" in
  standalone)
    XIAO_SOURCE=${2:-./xiao}
    XIAOD_SOURCE=${3:-./xiaod}
    [ -x "$XIAO_SOURCE" ] && [ -x "$XIAOD_SOURCE" ] || {
      echo "Usage: install-client.sh standalone [/path/to/xiao] [/path/to/xiaod]" >&2
      exit 2
    }
    install -m 0755 "$XIAO_SOURCE" "$PREFIX/bin/xiao"
    install -m 0755 "$XIAOD_SOURCE" "$PREFIX/bin/xiaod"
    echo "Installed xiao and xiaod."
    echo "Next: xiao quickstart"
    ;;
  pair)
    XIAO_SOURCE=${2:-./xiao}
    PAIRING_FILE=${3:-}
    CFG="$HOME/.config/xiao/client.toml"
    [ -x "$XIAO_SOURCE" ] && [ -n "$PAIRING_FILE" ] && [ -f "$PAIRING_FILE" ] || {
      echo "Usage: install-client.sh pair [/path/to/xiao] /path/to/pairing.toml" >&2
      exit 2
    }
    install -m 0755 "$XIAO_SOURCE" "$PREFIX/bin/xiao"
    mkdir -p "$(dirname "$CFG")"
    umask 077
    cp "$PAIRING_FILE" "$CFG"
    chmod 600 "$CFG"
    echo "Installed the non-root xiao client with private loopback pairing config."
    ;;
  *)
    echo "Usage: install-client.sh <standalone|pair> ..." >&2
    exit 2
    ;;
esac
