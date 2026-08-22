#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
case "${1:-}" in
  ''|--static-only) ;;
  *) printf 'usage: %s [--static-only]\n' "$0" >&2; exit 2 ;;
esac
pass(){ printf 'PASS  %s\n' "$1"; }
fail(){ printf 'FAIL  %s\n' "$1" >&2; exit 1; }

if [ ! -f Cargo.toml ] || [ ! -f src/main.rs ]; then
  fail 'Rust project layout'
fi
pass 'Rust project layout'
rg -q 'GITHUB_ACTIONS' packaging/build-module.sh || fail 'GitHub-Actions-only packaging guard'
pass 'Module packaging is guarded to GitHub Actions'
{
  rg -q 'owner_principal' src/storage/mod.rs &&
    rg -q 'cross_principal_operations_are_rejected' src/storage/mod.rs &&
    rg -q 'principal_cannot_switch_to_or_list_another_principals_sessions' src/session/mod.rs
} || fail 'Session principal ownership'
pass 'Session principal ownership enforced in storage/service tests'
rg -q 'get_updates\(offset, 50\)' src/telegram/mod.rs || fail 'Telegram long polling'
! rg -q 'setWebhook|webhook' src/telegram src/main.rs || fail 'No Telegram webhook in v0.1.0'
pass 'Telegram long-polling-only transport'
rg -q 'allowed\(message.chat.id' src/telegram/mod.rs || fail 'Telegram message ACL'
rg -q 'allowed\(message.chat.id, Some\(callback.from.id\)' src/telegram/mod.rs || fail 'Telegram callback ACL'
pass 'Telegram ACL before dispatch paths'
{
  rg -q 'enqueue_telegram_update' src/telegram/mod.rs &&
    rg -q 'spawn_update\(update\)' src/telegram/mod.rs &&
    rg -q 'long_generation_does_not_block_stop_other_principal_or_callbacks' src/telegram/mod.rs &&
    rg -q 'principal_locks' src/telegram/mod.rs
} || fail 'Telegram async update dispatch/E2E regression'
pass 'Telegram intake is decoupled from generation with stop/callback regression coverage'
{
  rg -q 'status=.interrupted' src/storage/mod.rs &&
    rg -q 'accepted_but_unclaimed_update_replays_after_restart' src/storage/mod.rs
} || fail 'Telegram durable inbox semantics'
pass 'Telegram durable inbox avoids silent-loss/destructive auto-replay'
rg -q 'sendRichMessageDraft' src/telegram/client.rs || fail 'Rich draft progress'
{
  rg -q 'with_optional' src/telegram/client.rs &&
    rg -q 'absent_optional_fields_are_omitted_not_serialized_as_null' src/telegram/client.rs &&
    ! rg -q '"reply_markup":markup' src/telegram/client.rs
} || fail 'Telegram optional payload fields'
pass 'Telegram omits absent reply markup instead of sending invalid JSON null'
rg -q 'Duration::from_millis\(750\)' src/telegram/mod.rs || fail 'Progress throttle'
rg -q 'HEARTBEAT: Duration = Duration::from_secs\(20\)' src/telegram/mod.rs || fail 'Progress heartbeat'
pass 'Ephemeral throttled progress path'
{
  rg -q 'View::from_markdown' src/telegram/mod.rs &&
    rg -q 'RichTable' src/presentation.rs &&
    rg -q 'markdown_final_becomes_native_rich_blocks' src/telegram/rich.rs
} || fail 'Rich final presentation parser'
pass 'Final Markdown-like output is parsed into native rich presentation blocks'
{
  rg -q 'expected_revision' src/telegram/mod.rs &&
    rg -q 'retire_keyboard' src/telegram/menu.rs
} || fail 'Menu stale/fallback protection'
pass 'Menu revision and edit/replacement fallback'
rg -q 'SIDE CHAT SESSION' src/telegram src/command src/session || fail 'Side marker'
rg -q 'side_never_writes_main' src/session/mod.rs || fail 'Side isolation test'
pass 'Side-chat isolation implementation'
{
  rg -q 'activate_account' src/storage/mod.rs &&
    rg -q 'UseAccount' src/command/mod.rs &&
    rg -q 'use_account_no_models_rolls_back_all_session_fields' src/command/mod.rs
} || fail 'Atomic account activation'
! rg -q 'SetAccount' src/command/mod.rs || fail 'Legacy non-atomic SetAccount path remains'
pass 'Login/account activation is atomic across provider/account/model'
{
  rg -q 'ANTIGRAVITY_CLIENT_ID' src/auth/mod.rs &&
    rg -q 'ANTIGRAVITY_OAUTH_REDIRECT_URI' src/auth/mod.rs &&
    rg -q 'loadCodeAssist' src/auth/mod.rs &&
    rg -q 'onboardUser' src/auth/mod.rs
} || fail 'CLIProxyAPI-compatible Antigravity OAuth flow'
{
  rg -q 'CODEX_OAUTH_AUTHORIZE_URL' src/auth/mod.rs &&
    rg -q 'codex_cli_simplified_flow' src/auth/mod.rs &&
    rg -q 'code_challenge_method' src/auth/mod.rs
} || fail 'CLIProxyAPI-compatible Codex OAuth flow'
pass 'Codex and Antigravity OAuth follow CLIProxyAPI browser login contracts'
{
  rg -q 'ToolRouter' src/agent/mod.rs src/tools/mod.rs &&
    rg -q 'ProviderStep::ToolCalls' src/agent/mod.rs &&
    rg -q 'typed_tool_call_continues_provider_until_final_answer' src/agent/mod.rs
} || fail 'Typed tool loop'
! rg -n 'Command::new\("sh"\)|Command::new\("su"\)|/system/bin/sh|/bin/sh' src/agent src/providers src/command src/tools >/dev/null || fail 'Unrestricted model root shell found'
rg -q 'bytes_stream\(\)' src/providers/mod.rs || fail 'Incremental provider streaming'
pass 'Typed bounded tool loop and incremental provider streaming'
{
  rg -q 'must be loopback-only' src/config/mod.rs &&
    rg -q 'ct_eq' src/ipc/mod.rs
} || fail 'Authenticated loopback IPC'
pass 'Authenticated loopback IPC'
{
  rg -q 'ipc-client-token' src/ipc/mod.rs &&
    rg -q 'ipc-admin-token' src/ipc/mod.rs
} || fail 'Separate IPC credentials'
{
  rg -q 'authorized_admin' src/ipc/mod.rs &&
    rg -q 'authorized_client' src/ipc/mod.rs
} || fail 'IPC role separation'
pass 'Separate client/admin IPC credentials'
{
  rg -q 'XIAO_MODULE_WRAPPER=1' module/termux/xiao-wrapper &&
    rg -q 'install_termux_wrappers' module/customize.sh module/service.sh module/action.sh &&
    rg -q 'remove_termux_wrappers' module/uninstall.sh
} || fail 'Managed Termux wrapper lifecycle'
pass 'Termux wrapper is installed automatically and removed/restored safely'
{
  rg -q 'token_configured' src/ipc/mod.rs &&
    ! rg -q '"token"[[:space:]]*:[[:space:]]*bot' src/ipc/mod.rs &&
    rg -q 'redact_text' src/ipc/mod.rs
} || fail 'Secret presence-only snapshot/redaction'
pass 'Snapshot exposes secret presence only and logs/errors are redacted'
pass 'No unrestricted AI root shell'
{
  rg -q 'normalize_cli' src/bin_cli.rs &&
    rg -q 'multiple_command_arguments_are_preserved' src/bin_cli.rs
} || fail 'Termux command alias arguments'
pass 'Termux command aliases preserve arguments before chat fallback'
{
  rg -q 'quickstart' src/bin_cli.rs &&
    rg -q 'configure_detached' src/standalone.rs &&
    rg -q 'libc::setsid' src/standalone.rs
} || fail 'Standalone quickstart/lifecycle'
{
  rg -q 'quickstart_initialization_is_idempotent_and_does_not_overwrite' src/standalone.rs &&
    rg -q 'stale_or_identity_mismatched_pid_state_is_removed_without_signaling' src/standalone.rs
} || fail 'Standalone lifecycle regression coverage'
{
  rg -q 'provision_client_config' src/standalone.rs &&
    ! rg -n 'println!.*token|dbg!.*token' src/bin_cli.rs src/standalone.rs >/dev/null
} || fail 'Standalone secret provisioning'
pass 'Standalone quickstart is idempotent, detached, identity-guarded and secret-safe'
{
  rg -q 'NeedsLogin' src/providers/mod.rs &&
    rg -q 'GatewayStatus::Degraded' src/command/mod.rs
} || fail 'Aggregate/provider health model'
pass 'Gateway/provider health distinguishes readiness from daemon liveness'
[ "$(rg -n '"compact"[[:space:]]*=>' src/command/mod.rs | wc -l)" -eq 0 ] || fail 'Fake /compact command still exposed'
pass 'No fake /compact command is exposed'
{
  [ -f module/webroot/index.html ] &&
    rg -q 'id="gateway"' module/webroot/index.html &&
    rg -q 'id="daemon"' module/webroot/index.html &&
    rg -q 'id="botToken"' module/webroot/index.html &&
    rg -q 'id="chatId"' module/webroot/index.html &&
    rg -q 'id="fetchModels"' module/webroot/index.html &&
    rg -q "const ACTION = '/data/adb/modules/xiao/action.sh'" module/webroot/assets/app.js &&
    rg -Fq 'run(`${ACTION} snapshot`)' module/webroot/assets/app.js &&
    ! rg -q 'agyClient|userIds|progressDetail|closeBehavior|customHeaders' module/webroot/index.html module/webroot/assets/app.js
} || fail 'WebUI gateway/daemon surfaces'
pass 'WebUI is limited to gateway/daemon status, two-field Telegram setup and custom model discovery'
{
  rg -q '/v1/admin/custom/models' src/ipc/mod.rs src/bin_cli.rs &&
    rg -q 'custom_model_catalog_is_sorted_deduplicated' src/ipc/mod.rs &&
    rg -q 'provider_api_key' src/auth/mod.rs src/providers/mod.rs
} || fail 'Custom provider model discovery'
pass 'Custom provider discovers models through authenticated root admin IPC'
{
    rg -q 'auto_restart_enabled' module/watchdog.sh module/common.sh &&
    rg -q '2097152' module/common.sh &&
    rg -q 'rotate_xiao_log.*XIAO_WATCHDOG_LOG' module/common.sh &&
    rg -q "trap 'cleanup'" module/watchdog.sh &&
    rg -q 'HOME="\$XIAO_DATA_DIR" XIAO_HOME="\$XIAO_DATA_DIR"' module/watchdog.sh &&
    rg -q 'XIAO_CONFIG="\$XIAO_CONFIG" XIAO_CLIENT_CONFIG="\$XIAO_CLIENT_CONFIG"' module/watchdog.sh
} || fail 'KernelSU watchdog hardening'
pass 'KernelSU watchdog has boot-safe environment, graceful stop, backoff, auto-restart and bounded logs'
{
    rg -q 'XIAO_RESTART=' module/common.sh &&
    rg -q 'restart_daemon' module/action.sh &&
    rg -q 'Restart xiaod diminta melalui watchdog' module/action.sh &&
    rg -q 'karena restart diminta; mulai ulang sekarang' module/watchdog.sh &&
    rg -q 'waitForDaemon' module/webroot/assets/app.js &&
    ! sed -n '/restart)/,/;;/p' module/action.sh | rg -q 'stop_watchdog'
} || fail 'WebUI-safe daemon restart handshake'
pass 'WebUI restarts only the watchdog child and waits for replacement readiness'
for f in module/*.sh module/termux/xiao-wrapper scripts/device-custom-e2e.sh; do sh -n "$f"; done
for f in packaging/build-module.sh scripts/acceptance.sh; do bash -n "$f"; done
pass 'Shell syntax'
node --check module/webroot/assets/app.js >/dev/null
node --check module/webroot/assets/ksu-bridge.js >/dev/null
pass 'WebUI JavaScript syntax'
python - <<'PY'
import tomllib
for p in [
    'config/config.example.toml',
    'config/config.termux-test.toml',
    'module/config.example.toml',
]:
    with open(p,'rb') as f: tomllib.load(f)
PY
pass 'Example TOML parses'

printf 'SKIP  Rust format/check/test/build run only in GitHub Actions.\n'
printf '\nStatic acceptance checks completed successfully. Device/provider integration items remain per docs/ACCEPTANCE.md.\n'
