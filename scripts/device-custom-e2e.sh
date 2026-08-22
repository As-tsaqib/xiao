#!/system/bin/sh
set -u

XIAO_MODULE=/data/adb/modules/xiao
CLIPROXY_CONFIG=/data/adb/cliproxyapi/config.yaml
CURL=/data/data/com.termux/files/usr/bin/curl
TEST_PORT=38931
TEST_MODEL=${XIAO_E2E_MODEL:-gpt-5.6-luna}

[ "$(id -u)" -eq 0 ] || { echo 'Run this test as root.' >&2; exit 1; }
[ -x "$XIAO_MODULE/bin/xiaod" ] || { echo 'Installed xiaod binary not found.' >&2; exit 1; }
[ -x "$CURL" ] || { echo 'Termux curl not found.' >&2; exit 1; }
[ -r "$CLIPROXY_CONFIG" ] || { echo 'CLIProxyAPI config not found.' >&2; exit 1; }
case "$TEST_MODEL" in
  ''|*[!A-Za-z0-9._:-]*) echo 'XIAO_E2E_MODEL has an unsupported shape.' >&2; exit 1 ;;
esac

test_dir=$(mktemp -d /data/adb/xiao-e2e.XXXXXX) || exit 1
daemon_pid=
cleanup() {
  result=$?
  trap - EXIT INT TERM
  if [ -n "$daemon_pid" ]; then
    kill -TERM "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  case "$test_dir" in
    /data/adb/xiao-e2e.*) rm -rf -- "$test_dir" ;;
  esac
  exit "$result"
}
trap cleanup EXIT INT TERM

api_key=$(awk '
  /^api-keys:[[:space:]]*$/ {
    getline
    sub(/^[[:space:]]*-[[:space:]]*/, "")
    gsub(/^"|"$/, "")
    print
    exit
  }
' "$CLIPROXY_CONFIG")
case "$api_key" in
  ''|'@GENERATED_API_KEY@'|*[!A-Za-z0-9._-]*)
    echo 'CLIProxyAPI active API key is unavailable or has an unsupported shape.' >&2
    exit 1
    ;;
esac

mkdir -p "$test_dir/data" "$test_dir/logs" "$test_dir/secrets" "$test_dir/tmp"
cp "$XIAO_MODULE/config.example.toml" "$test_dir/config.toml" || exit 1
sed -i \
  -e "s#/data/adb/xiao#$test_dir#g" \
  -e "s#127.0.0.1:37921#127.0.0.1:$TEST_PORT#g" \
  "$test_dir/config.toml"
chmod 0600 "$test_dir/config.toml"

HOME="$test_dir" XIAO_HOME="$test_dir" \
  XIAO_CONFIG="$test_dir/config.toml" XIAO_CLIENT_CONFIG="$test_dir/client.toml" \
  TMPDIR="$test_dir/tmp" XIAO_BOOT_START=1 \
  "$XIAO_MODULE/bin/xiaod" > "$test_dir/logs/daemon.log" 2>&1 &
daemon_pid=$!

attempt=0
while [ "$attempt" -lt 30 ] && [ ! -s "$test_dir/secrets/ipc-admin-token.secret" ]; do
  kill -0 "$daemon_pid" 2>/dev/null || {
    sed -n '1,120p' "$test_dir/logs/daemon.log" >&2
    exit 1
  }
  sleep 1
  attempt=$((attempt + 1))
done
[ -s "$test_dir/secrets/ipc-admin-token.secret" ] || { echo 'xiaod admin token was not created.' >&2; exit 1; }
admin_token=$(cat "$test_dir/secrets/ipc-admin-token.secret")

models_payload=$(printf '{"base_url":"http://127.0.0.1:8317/v1","api_key":"%s"}' "$api_key")
models_response=$($CURL -fsS --max-time 30 \
  -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' \
  --data "$models_payload" "http://127.0.0.1:$TEST_PORT/v1/admin/custom/models") || exit 1
printf '%s' "$models_response" | grep -Fq "\"$TEST_MODEL\"" || {
  echo "xiao model discovery did not return $TEST_MODEL." >&2
  exit 1
}

apply_payload=$(printf '{"gateway_enabled":true,"custom_enabled":true,"custom_name":"CLIProxyAPI E2E","custom_base_url":"http://127.0.0.1:8317/v1","custom_protocol":"openai_chat_completions","custom_models":["%s"],"custom_default_model":"%s","custom_api_key":"%s"}' "$TEST_MODEL" "$TEST_MODEL" "$api_key")
apply_response=$($CURL -fsS --max-time 30 \
  -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' \
  --data "$apply_payload" "http://127.0.0.1:$TEST_PORT/v1/admin/apply") || exit 1
printf '%s' "$apply_response" | grep -Fq '"ok":true' || { echo 'xiao rejected custom provider configuration.' >&2; exit 1; }

provider_response=$($CURL -fsS --max-time 30 \
  -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' \
  --data '{"principal":"device:e2e","input":"/provider custom"}' \
  "http://127.0.0.1:$TEST_PORT/v1/command") || exit 1
printf '%s' "$provider_response" | grep -Fq 'custom' || { echo 'xiao could not activate custom provider.' >&2; exit 1; }

chat_response=$($CURL -fsS --max-time 180 \
  -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' \
  --data '{"principal":"device:e2e","input":"Balas hanya dengan teks XIAO_E2E_OK"}' \
  "http://127.0.0.1:$TEST_PORT/v1/command") || exit 1
printf '%s' "$chat_response" | grep -Fq 'XIAO_E2E_OK' || {
  echo 'Custom endpoint returned a response, but the expected marker was absent.' >&2
  exit 1
}

printf 'PASS  xiaod boot-style environment started successfully\n'
printf 'PASS  xiao discovered custom model %s through CLIProxyAPI /v1/models\n' "$TEST_MODEL"
printf 'PASS  CLIProxyAPI custom model %s returned XIAO_E2E_OK through xiao CommandCore\n' "$TEST_MODEL"
