#!/system/bin/sh
# shellcheck disable=SC2034

if [ -z "${MODDIR:-}" ]; then
  MODDIR=${0%/*}
fi

# These shared constants are consumed by scripts that source this file.
XIAO_DATA_DIR=/data/adb/xiao
XIAO_CONFIG=$XIAO_DATA_DIR/config.toml
XIAO_CLIENT_CONFIG=$XIAO_DATA_DIR/client.toml
XIAO_DATA=$XIAO_DATA_DIR/data
XIAO_SECRETS=$XIAO_DATA_DIR/secrets
XIAO_LOG_DIR=$XIAO_DATA_DIR/logs
XIAO_CACHE=$XIAO_DATA_DIR/cache
XIAO_RUN_DIR=$XIAO_DATA_DIR/run
XIAO_IPC_DIR=$XIAO_DATA_DIR/ipc
XIAO_TMP_DIR=$XIAO_DATA_DIR/tmp
XIAO_DAEMON_LOG=$XIAO_LOG_DIR/daemon.log
XIAO_WATCHDOG_LOG=$XIAO_LOG_DIR/watchdog.log
XIAO_DAEMON_PID=$XIAO_RUN_DIR/xiaod.pid
XIAO_WATCHDOG_PID=$XIAO_RUN_DIR/watchdog.pid
XIAO_STOP=$XIAO_RUN_DIR/stop
XIAO_DISABLE=$XIAO_DATA_DIR/disable
XIAO_BINARY=$MODDIR/bin/xiao
XIAOD_BINARY=$MODDIR/bin/xiaod
XIAO_WATCHDOG=$MODDIR/watchdog.sh

xiao_log() {
  if [ "${XIAO_LOG_TO_FILE:-0}" = 1 ]; then
    rotate_xiao_log "$XIAO_WATCHDOG_LOG" 2>/dev/null || true
    printf '[xiao] %s\n' "$*" >> "$XIAO_WATCHDOG_LOG"
  else
    printf '[xiao] %s\n' "$*"
  fi
}

ensure_xiao_dirs() {
  umask 077
  mkdir -p "$XIAO_DATA_DIR" "$XIAO_DATA" "$XIAO_SECRETS" "$XIAO_LOG_DIR" \
    "$XIAO_CACHE" "$XIAO_RUN_DIR" "$XIAO_IPC_DIR" "$XIAO_TMP_DIR"
  chmod 0700 "$XIAO_DATA_DIR" "$XIAO_DATA" "$XIAO_SECRETS" "$XIAO_LOG_DIR" \
    "$XIAO_CACHE" "$XIAO_RUN_DIR" "$XIAO_IPC_DIR" "$XIAO_TMP_DIR" 2>/dev/null || true
  if [ ! -f "$XIAO_CONFIG" ]; then
    cp "$MODDIR/config.example.toml" "$XIAO_CONFIG" || return 1
  fi
  chmod 0600 "$XIAO_CONFIG" 2>/dev/null || true
}

run_xiao_admin() {
  HOME="$XIAO_DATA_DIR" XIAO_HOME="$XIAO_DATA_DIR" \
    XIAO_CONFIG="$XIAO_CONFIG" XIAO_CLIENT_CONFIG="$XIAO_CLIENT_CONFIG" \
    TMPDIR="$XIAO_TMP_DIR" \
    "$XIAO_BINARY" admin "$@"
}

valid_pid() {
  case "${1:-}" in
    ''|*[!0-9]*) return 1 ;;
  esac
  [ "$1" -gt 1 ] 2>/dev/null
}

pid_matches() {
  match_pid=$1
  match_text=$2
  valid_pid "$match_pid" || return 1
  [ -r "/proc/$match_pid/cmdline" ] || return 1
  kill -0 "$match_pid" 2>/dev/null || return 1
  tr '\000' '\n' < "/proc/$match_pid/cmdline" 2>/dev/null | grep -Fqx "$match_text"
}

pid_from_file() {
  [ -f "$1" ] || return 1
  sed -n '1p' "$1" 2>/dev/null
}

daemon_is_running() {
  daemon_pid=$(pid_from_file "$XIAO_DAEMON_PID" 2>/dev/null) || return 1
  pid_matches "$daemon_pid" "$XIAOD_BINARY"
}

watchdog_is_running() {
  watchdog_pid=$(pid_from_file "$XIAO_WATCHDOG_PID" 2>/dev/null) || return 1
  pid_matches "$watchdog_pid" "$XIAO_WATCHDOG"
}

wait_owned_exit() {
  wait_pid=$1
  wait_marker=$2
  wait_seconds=${3:-15}
  while [ "$wait_seconds" -gt 0 ] && pid_matches "$wait_pid" "$wait_marker"; do
    sleep 1
    wait_seconds=$((wait_seconds - 1))
  done
  ! pid_matches "$wait_pid" "$wait_marker"
}

stop_owned_pid_file() {
  stop_pid_file=$1
  stop_marker=$2
  stop_pid=$(pid_from_file "$stop_pid_file" 2>/dev/null || true)
  if pid_matches "$stop_pid" "$stop_marker"; then
    kill -TERM "$stop_pid" 2>/dev/null || true
    if ! wait_owned_exit "$stop_pid" "$stop_marker" 15; then
      kill -KILL "$stop_pid" 2>/dev/null || true
      wait_owned_exit "$stop_pid" "$stop_marker" 2 || true
    fi
  fi
  rm -f "$stop_pid_file"
}

auto_restart_enabled() {
  [ ! -f "$XIAO_CONFIG" ] && return 0
  restart_value=$(awk '
    /^\[gateway\][[:space:]]*$/ { in_gateway=1; next }
    /^\[/ { in_gateway=0 }
    in_gateway && /^[[:space:]]*auto_restart[[:space:]]*=/ {
      sub(/^[^=]*=[[:space:]]*/, ""); sub(/[[:space:]#].*$/, ""); print; exit
    }
  ' "$XIAO_CONFIG" 2>/dev/null)
  [ "$restart_value" != false ]
}

ipc_bind() {
  bind_value=$(awk '
    /^\[ipc\][[:space:]]*$/ { in_ipc=1; next }
    /^\[/ { in_ipc=0 }
    in_ipc && /^[[:space:]]*bind[[:space:]]*=/ {
      sub(/^[^=]*=[[:space:]]*/, ""); gsub(/^"|"$/, ""); print; exit
    }
  ' "$XIAO_CONFIG" 2>/dev/null)
  case "$bind_value" in
    127.0.0.1:*|localhost:*)
      bind_port=${bind_value##*:}
      case "$bind_port" in
        ''|*[!0-9]*) printf '127.0.0.1:37921\n' ;;
        *) printf '%s\n' "$bind_value" ;;
      esac
      ;;
    *)
      printf '127.0.0.1:37921\n'
      ;;
  esac
}

ensure_client_config() {
  client_token_file=$XIAO_SECRETS/ipc-client-token.secret
  [ -s "$client_token_file" ] || return 1
  client_token=$(cat "$client_token_file" 2>/dev/null) || return 1
  [ -n "$client_token" ] || return 1
  client_temp=$XIAO_RUN_DIR/client.toml.$$
  {
    printf '# Managed by xiao module\n'
    printf 'endpoint = "http://%s"\n' "$(ipc_bind)"
    printf 'token = "%s"\n' "$client_token"
    printf 'principal = "termux:default"\n'
  } > "$client_temp" || return 1
  chmod 0600 "$client_temp" 2>/dev/null || true
  mv -f "$client_temp" "$XIAO_CLIENT_CONFIG"
  chmod 0600 "$XIAO_CLIENT_CONFIG" 2>/dev/null || true
}

rotate_xiao_log() {
  rotate_file=$1
  [ -f "$rotate_file" ] || return 0
  rotate_size=$(wc -c < "$rotate_file" 2>/dev/null || printf '0')
  case "$rotate_size" in ''|*[!0-9]*) return 0 ;; esac
  if [ "$rotate_size" -gt 2097152 ]; then
    tail -c 1048576 "$rotate_file" > "$rotate_file.tmp" 2>/dev/null &&
      mv -f "$rotate_file.tmp" "$rotate_file"
  fi
}
