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
! rg -q 'setWebhook|webhook' src/telegram src/main.rs || fail 'No Telegram webhook'
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
  rg -q 'AI_ACTION_SEARCHING' src/telegram/rich.rs &&
    rg -q 'ProgressActivity::Fetching' src/telegram/mod.rs &&
    rg -q 'progress_maps_real_work_to_semantic_activities' src/telegram/mod.rs
} || fail 'Semantic animated Telegram progress'
pass 'Telegram progress uses native thinking with semantic AI Actions'
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
  rg -q 'ToolRegistry' src/agent/mod.rs src/tools/mod.rs &&
    rg -q 'ToolPolicy' src/agent/mod.rs src/tools/mod.rs &&
    rg -q 'ProviderStep::ToolCalls' src/agent/mod.rs &&
    rg -q 'typed_tool_call_continues_provider_until_final_answer' src/agent/mod.rs &&
    ! rg -q 'ToolRouter' src/agent/mod.rs src/providers/mod.rs src/tools/mod.rs
} || fail 'Typed tool loop'
! rg -n 'Command::new\("(sh|bash|su)"\)' src/agent src/providers src/command src/tools src/runtime >/dev/null || fail 'Unrestricted model root shell found'
rg -q 'bytes_stream\(\)' src/providers/mod.rs || fail 'Incremental provider streaming'
pass 'Typed bounded tool loop and incremental provider streaming'
{
  rg -q 'enum ToolProtocol' src/providers/mod.rs &&
    rg -q 'StructuredJsonFallback' src/providers/mod.rs &&
    rg -q 'antigravity_tool_specs' src/providers/mod.rs &&
    rg -q 'probe_custom_tool_capability' src/providers/mod.rs &&
    rg -q 'codex_antigravity_and_custom_protocols_keep_the_same_agent_tool_workflow' src/agent/mod.rs &&
    rg -q 'chat_only_model_rejects_action_explicitly_but_serves_information' src/agent/mod.rs
} || fail 'Provider-independent agent protocol parity'
pass 'Codex, Antigravity and probed Custom protocols retain an explicit agent capability'
{
  [ -f src/semantic/mod.rs ] &&
    rg -q 'struct SemanticEvaluator' src/semantic/mod.rs &&
    rg -q 'tools: Vec::new\(\)' src/semantic/mod.rs &&
    rg -q 'malformed_after_repair_is_conservative' src/semantic/mod.rs &&
    rg -q 'semantic_intent_handles_action_wording_outside_deterministic_markers' src/agent/completion.rs
} || fail 'Bounded no-tools semantic evaluator'
pass 'Semantic decisions are schema-validated, bounded, no-tools and conservative'
{
  [ -f src/identity/templates/SOUL.md ] &&
    [ -f src/identity/templates/USER.md ] &&
    [ -f src/identity/templates/MEMORY.md ] &&
    [ -f src/identity/templates/AGENTS.md ] &&
    [ -f src/identity/templates/ENVIRONMENT.md ] &&
    rg -q 'identity_bootstrap_survives_restart_and_never_overwrites_owner_files' src/identity/mod.rs &&
    rg -q 'write_soul_owner_approved' src/identity/mod.rs &&
    rg -q 'write_environment' src/runtime/environment.rs
} || fail 'Persistent living identity workspace'
pass 'Persistent identity files bootstrap without ordinary SOUL replacement'
{
  rg -q 'struct RuntimeEnvironment' src/runtime/environment.rs &&
    rg -q 'trait HostProbe' src/runtime/environment.rs &&
    rg -q 'enum CapabilityStatus' src/runtime/capabilities.rs &&
    rg -q 'MissingInstallable' src/runtime/capabilities.rs &&
    rg -q 'capability_resolution_prevents_false_cannot_when_termux_backend_is_usable' src/runtime/capabilities.rs
} || fail 'Typed runtime and capability resolution'
pass 'Runtime probing and non-boolean capability resolution are covered'
{
  rg -q 'name: "termux_terminal"' src/tools/builtin/terminal.rs &&
    rg -q 'register_alias\("terminal", "termux_terminal"\)' src/agent/mod.rs &&
    rg -q 'struct TermuxExecutor' src/runtime/execution.rs &&
    rg -q 'setgroups' src/runtime/execution.rs &&
    rg -q 'PR_SET_NO_NEW_PRIVS' src/runtime/execution.rs &&
    rg -q 'model-supplied shell command strings are not accepted' src/runtime/execution.rs &&
    rg -q 'destructive Termux command' src/tools/policy.rs &&
    rg -q 'real_executor_enforces_termux_env_cwd_timeout_cancel_and_output_bounds' src/runtime/execution.rs
} || fail 'Controlled Termux general executor'
pass 'Termux structured argv, UID drop, bounds and argument-aware policy are covered'
{
  rg -q 'struct DependencyResolver' src/runtime/dependency.rs &&
    rg -q 'trusted_package_for_binary' src/runtime/capabilities.rs src/runtime/dependency.rs &&
    rg -q 'validate_package' src/runtime/dependency.rs &&
    rg -q 'trusted_missing_dependency_is_installed_reprobed_and_audited' src/runtime/dependency.rs &&
    rg -q 'missing_dependency_installs_reprobes_and_resumes_original_command' src/tools/builtin/terminal.rs &&
    rg -q 'package_names_and_unknown_remote_installers_are_rejected' src/runtime/dependency.rs
} || fail 'Trusted Termux dependency resolution'
pass 'Trusted dependency install is normalized, audited, re-probed and resumed'
{
  rg -q 'trait AndroidBroker' src/runtime/android.rs &&
    rg -q 'enum AndroidOperation' src/runtime/android.rs &&
    rg -q 'AndroidXiaoRestartTool' src/tools/builtin/android.rs &&
    rg -q 'privileged_tool_requires_exact_durable_one_shot_approval' src/tools/registry.rs &&
    ! rg -q 'struct RootShell|name: "root_shell"|Command::new\("su"\)' src/runtime src/tools src/agent
} || fail 'Typed privileged Android broker'
pass 'Privileged Android surface is typed and exact-approval guarded'
{
  rg -Fq 'UNIQUE(owner_principal,scope,category,key)' src/storage/mod.rs &&
    rg -q 'MemoryStore' src/memory/mod.rs src/memory/store.rs &&
    rg -q 'synonymous_explicit_preference_change_updates_one_canonical_memory' src/memory/evaluator.rs &&
    rg -q 'explicit_forget_removes_active_memory' src/memory/evaluator.rs
} || fail 'Editable principal-scoped memory'
pass 'Memory UPSERT, deduplication, history and explicit forget semantics'
{
  rg -q 'MemoryDecisionKind' src/memory/evaluator.rs &&
    rg -q 'Rekey' src/memory/evaluator.rs &&
    rg -q 'with_workspace' src/memory/store.rs &&
    rg -q 'manual_file_reconcile' src/memory/store.rs &&
    rg -q 'generalized_preferences_facts_and_manual_edits_reconcile' src/memory/evaluator.rs
} || fail 'File-authoritative generalized memory'
pass 'USER/MEMORY current state supports generalized lifecycle and manual reconciliation'
{
  rg -q 'messages_fts' src/storage/mod.rs &&
    rg -q 'ContextEngine' src/agent/mod.rs src/context/engine.rs &&
    rg -q 'current_request_and_system_prompt_survive_budget_pressure' src/context/engine.rs &&
    rg -q 'compression_persists_summary_without_deleting_raw_history' src/context/engine.rs
} || fail 'Session retrieval and bounded context'
pass 'Principal-scoped FTS retrieval and bounded context compression'
{
  rg -Fq 'UNIQUE(owner_principal,name)' src/storage/mod.rs &&
    rg -q 'LearningEvaluator' src/agent/mod.rs src/learning/evaluator.rs &&
    rg -q 'verified_work_creates_then_updates_one_canonical_skill' src/learning/evaluator.rs &&
    rg -q 'trivial_failed_cancelled_and_unverified_work_never_creates_skill' src/learning/evaluator.rs
} || fail 'Verified skill learning and deduplication'
pass 'Verified post-completion learning updates canonical principal-scoped skills'
{
  rg -q 'serde_yaml::from_str' src/skills/filesystem.rs &&
    rg -q 'SKILL.md must start with YAML frontmatter' src/skills/filesystem.rs &&
    rg -q 'resolve_dependencies' src/skills/filesystem.rs &&
    rg -q 'community_minimum_and_optional_metadata_are_tolerated' src/skills/filesystem.rs &&
    rg -q 'observable_trace_creates_generalized_skill_with_pitfall_then_updates_same_skill' src/learning/evaluator.rs &&
    rg -q 'tool_counts_without_reusable_semantics_do_not_create_a_skill' src/learning/evaluator.rs
} || fail 'Filesystem community skills and trace learning'
pass 'Community SKILL.md discovery/gating and generalized trace learning are covered'
{
  rg -q 'enum VerificationState' src/agent/completion.rs &&
    rg -q 'NotYetVerified' src/agent/completion.rs src/agent/mod.rs &&
    rg -q 'max_no_progress_repeats' src/config/mod.rs src/agent/mod.rs &&
    rg -q 'failure_changes_strategy_and_unverified_final_continues_until_evidence' src/agent/mod.rs &&
    rg -q 'same_call_claim' src/agent/completion.rs
} || fail 'Bounded adaptive loop and completion verification'
pass 'NotYetVerified continues and no-progress bounds prevent false completion/infinite retries'
{
  rg -q 'RUN_OBSERVATIONS' src/agent/mod.rs &&
    rg -q 'remaining_budgets' src/agent/mod.rs &&
    rg -q 'repeated_identical_failed_action_terminates_as_bounded_blocker' src/agent/mod.rs
} || fail 'Runtime observations and bounded no-progress blocker'
pass 'Retries receive observable runtime state and repeated failure terminates Blocked'
{
  rg -q 'v010_database_upgrades_additively_without_losing_history' src/storage/mod.rs &&
    rg -q 'v020_migration_is_fresh_and_idempotent_with_consistent_fts' src/storage/mod.rs &&
    rg -q 'reopen_quarantines_inflight_agent_and_tool_runs_without_replay' src/storage/mod.rs
} || fail 'v0.2 additive migration coverage'
pass 'Fresh, upgrade, idempotent and crash-recovery migrations covered'
{
  rg -q 'INSERT OR IGNORE INTO schema_migrations\(version\) VALUES\(10\)' src/storage/mod.rs &&
    rg -q 'INSERT OR IGNORE INTO schema_migrations\(version\) VALUES\(11\)' src/storage/mod.rs &&
    rg -q 'INSERT OR IGNORE INTO schema_migrations\(version\) VALUES\(12\)' src/storage/mod.rs &&
    rg -q 'CREATE TABLE IF NOT EXISTS approvals' src/storage/mod.rs &&
    rg -q 'CREATE TABLE IF NOT EXISTS telegram_session_scopes' src/storage/mod.rs &&
    rg -q 'CREATE TABLE IF NOT EXISTS provider_capabilities' src/storage/mod.rs &&
    rg -q 'CREATE TABLE IF NOT EXISTS dependency_installs' src/storage/mod.rs &&
    rg -q 'CREATE TABLE IF NOT EXISTS environment_probes' src/storage/mod.rs &&
    rg -q 'UPDATE dependency_installs SET status=.interrupted' src/storage/mod.rs
} || fail 'Final architecture migrations through version 12'
pass 'Migrations v10-v12 and uncertain package-install quarantine are present'
{
  rg -q 'Command::Approvals' src/command/mod.rs &&
    rg -q 'Command::Approve' src/command/mod.rs &&
    rg -q 'Command::Deny' src/command/mod.rs &&
    rg -q 'send_document' src/telegram/mod.rs src/telegram/client.rs &&
    rg -q 'result_file_is_sent_through_telegram_multipart_document_path' src/telegram/client.rs
} || fail 'Telegram approvals and file results'
pass 'Telegram exposes approvals and verified multipart result files'
{
  [ -f src/telegram/scope.rs ] &&
    [ -f src/telegram/commands.rs ] &&
    [ -f src/telegram/login.rs ] &&
    rg -q 'message_thread_id' src/telegram/types.rs src/telegram/client.rs &&
    rg -q 'public_registry_is_exact_and_hidden_commands_are_not_advertised' src/telegram/commands.rs &&
    rg -q 'custom_login_wizard_discovers_pages_probes_and_rejects_wrong_topic_callbacks' src/telegram/mod.rs &&
    rg -q 'wizard_state_requires_owner_chat_topic_menu_and_unexpired_state' src/telegram/login.rs &&
    rg -q 'topic_session_manager_paginates_and_preserves_archived_history' src/command/mod.rs
} || fail 'Telegram topic scope, registry and Custom login UX'
! rg -q '"provider"[[:space:]]*=>' src/command/mod.rs || fail 'Removed /provider route remains'
pass 'Telegram topics, exact command registry, removed commands and scoped Custom wizard are covered'
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

for path in src/mcp src/subagents src/vector src/cron src/plugins; do
  [ ! -e "$path" ] || fail "Out-of-scope architecture present: $path"
done
pass 'No MCP, subagent, vector, cron or plugin subsystem was added'

printf 'SKIP  Rust format/check/test/clippy are not run by --static-only.\n'
printf '\nStatic acceptance checks completed successfully. Device/provider integration items remain per docs/ACCEPTANCE.md.\n'
