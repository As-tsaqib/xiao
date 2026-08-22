#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
require_cargo=0
[ "${1:-}" = "--require-cargo" ] && require_cargo=1
pass(){ printf 'PASS  %s\n' "$1"; }
fail(){ printf 'FAIL  %s\n' "$1" >&2; exit 1; }

if [ ! -f Cargo.toml ] || [ ! -f src/main.rs ]; then
  fail 'Rust project layout'
fi
pass 'Rust project layout'
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
  rg -q 'oauth_client_id' src/config/mod.rs &&
    rg -q 'agyClientId' webui/index.html
} || fail 'Deployable Antigravity configuration'
! rg -q 'XIAO_AGY_CLIENT_ID' src config webui module docs README.md || fail 'Legacy Antigravity environment-variable dependency remains'
pass 'Antigravity OAuth is configurable through daemon/WebUI without shell env dependency'
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
! rg -n '\bsu\b' src/bin_cli.rs termux/install-client.sh >/dev/null || fail 'Termux normal client contains su'
pass 'Termux normal path has no su'
{
  rg -q 'masked_token' src/ipc/mod.rs &&
    rg -q 'redact_text' src/ipc/mod.rs
} || fail 'Secret masking/redaction'
pass 'Secret masking and redacted logs'
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
  [ -f webui/index.html ] &&
    rg -q 'id="gateway"' webui/index.html &&
    rg -q 'id="daemon"' webui/index.html
} || fail 'WebUI gateway/daemon surfaces'
pass 'WebUI Gateway and Daemon surfaces'
{
  rg -q 'auto_restart_enabled' module/supervisor.sh &&
    rg -q '2097152' module/supervisor.sh &&
    rg -q 'trap.*cleanup' module/supervisor.sh
} || fail 'KernelSU supervisor hardening'
pass 'KernelSU supervisor has graceful stop, backoff policy, auto-restart policy and bounded logs'
for f in module/*.sh termux/install-client.sh; do sh -n "$f"; done
for f in packaging/*.sh scripts/acceptance.sh; do bash -n "$f"; done
pass 'Shell syntax'
node --check webui/assets/app.js >/dev/null
node --check webui/assets/ksu-bridge.js >/dev/null
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

if command -v cargo >/dev/null 2>&1; then
  cargo fmt --all -- --check
  cargo check --locked --all-targets --all-features
  cargo test --locked --all-targets --all-features
  cargo clippy --locked --all-targets --all-features -- -D warnings
  cargo build --locked --release --all-features
  [ -f Cargo.lock ] || fail 'Cargo.lock missing after Cargo validation'
  pass 'Rust format/check/tests/clippy/release build'
elif [ "$require_cargo" -eq 1 ]; then
  fail 'Cargo required but not installed'
else
  printf 'SKIP  Cargo unavailable in this environment; CI is the compile/test authority.\n'
fi
printf '\nStatic acceptance checks completed successfully. Device/provider integration items remain per docs/ACCEPTANCE.md.\n'
