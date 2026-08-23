# Xiao v0.2.0 Acceptance Coverage

This document maps the v0.2.0 specification to implemented automated and
device validation. `scripts/acceptance.sh --static-only` checks source/package
invariants without invoking Cargo. Rust gates are also runnable locally and in
CI. Tests requiring a real Android reboot, Telegram credential, or provider
account remain explicit device checks and are never reported as automated
success.

## Agent and tools

| Requirement | Coverage |
|---|---|
| Registry-driven tools; no provider-owned discovery | `ToolRegistry`, `ToolSpec`, provider wire-translation test, static rejection of `ToolRouter` |
| Duplicate/unknown/malformed calls fail safely | Registry duplicate/unknown tests, strict argument deserialization, malformed `context_stats` test |
| Basic policy boundary | Read-only allow, explicit memory side-effect allowlist, destructive policy-denial test |
| Bounded turns, timeout, and output | `max_turn_guard_fails_run_without_persisting_assistant`; registry timeout/output test |
| Cancellation at provider/tool boundaries | `stop_cancels_active_generation`; `cancellation_during_tool_marks_both_run_boundaries_terminal` |
| Captured final-write target | `concurrent_session_switch_cannot_redirect_captured_final_write` |
| Durable run/tool audit | Typed loop asserts completed `agent_runs` and succeeded `tool_runs`; unknown tool asserts durable `denied` row |
| Crash uncertainty | `reopen_quarantines_inflight_agent_and_tool_runs_without_replay` |
| No unrestricted shell | No shell implementation/spec; policy denies privileged/destructive tools; static acceptance grep |

## Memory

| Requirement | Coverage |
|---|---|
| Principal-scoped canonical uniqueness | SQLite `UNIQUE(owner_principal,scope,category,key)` plus cross-principal search/delete test |
| UPSERT current state | `create_upsert_alias_and_delete_keep_one_active_state_with_history` |
| Concise → detailed remains one active row | `synonymous_explicit_preference_change_updates_one_canonical_memory` |
| Generic explicit fact update | `generic_explicit_fact_changes_and_forgets_same_canonical_state` |
| Explicit forget | Dedicated response-style and generic deletion tests; deletion history retained separately |
| FTS retrieval | Principal-scoped `MemoryStore::search` test and trigger-backed `memories_fts` |
| Secret rejection | Sensitive value/identity tests plus structured redaction and token-pattern tests |
| Conservative implicit memory | Evaluator only accepts a narrow durable project fact after verified completion |

## Session retrieval and context

| Requirement | Coverage |
|---|---|
| Principal-scoped FTS5 session search | `fts_search_is_relevant_bounded_and_principal_scoped` |
| Result count/content bound | Search clamps limit and truncates/redacts each result; test uses oversized content |
| Memory in context | `user_and_agent_memory_enter_delimited_context` |
| Relevant skill progressive disclosure | `only_relevant_selected_skill_is_progressively_disclosed` |
| Character-budget context | `current_request_and_system_prompt_survive_budget_pressure` |
| Protected system/current request | Same pressure test asserts exact first and last protected messages |
| Durable compaction without raw deletion | `compression_persists_summary_without_deleting_raw_history` |
| MAIN/SIDE behavior preserved | Existing `side_never_writes_main`, no-nesting, ownership and Telegram renderer tests |

## Skills, completion, and learning

| Requirement | Coverage |
|---|---|
| Complete skill shape and ownership | Schema/store require summary, when-to-use, procedure, pitfalls, verification; isolation test |
| FTS search and full view | `skill_search`/`skill_view` backed by principal-filtered `SkillRegistry` |
| `running → verifying → completed` | Agent status transitions plus `CompletionVerifier` observable evidence |
| Recovery can resolve earlier failure | `unresolved_failure_is_not_verified_but_successful_recovery_is` |
| Verified reusable work creates skill | `verified_work_creates_then_updates_one_canonical_skill` |
| Near-synonym updates canonical skill | `related_skill_updates_canonical_row_instead_of_creating_duplicate` |
| Failed/cancelled/interrupted/trivial work learns no skill | `trivial_failed_cancelled_and_unverified_work_never_creates_skill` |
| Failed attempts can become pitfalls after success | Learning trace derives non-success observations into candidate pitfalls only after verified completion |
| No hidden reasoning persisted | `LearningTrace` contains only goal, safe observations, final result, and verification evidence |

## Migrations and regression

| Requirement | Coverage |
|---|---|
| Fresh v0.2.0 database | `v020_migration_is_fresh_and_idempotent_with_consistent_fts` checks all new objects/version 9 |
| Upgrade existing v0.1.0 database | `v010_database_upgrades_additively_without_losing_history` verifies owner assignment, raw history, and FTS backfill |
| Repeated migrate is safe | Fresh migration test calls `migrate()` twice after data insertion and asserts one FTS hit |
| WAL/foreign keys/reopen durability | Existing storage reopen and inbox tests remain green |
| Telegram ACL/inbox/responsiveness | Existing ACL-before-dispatch, durable inbox, and slow-generation adapter regression tests |
| Provider/account compatibility | Existing Codex/Antigravity/custom protocol, OAuth, refresh, atomic account/model tests |
| Loopback IPC and redaction | Existing non-loopback rejection, constant-time bearer, split privilege, snapshot/log tests |
| Formatting/build/tests/lints | `cargo fmt --all -- --check`, `cargo check`, `cargo test`, `cargo clippy --all-targets --all-features -- -D warnings` |

## Device integration checklist

After CI produces the Android arm64 module archive:

1. Flash `xiao-v0.2.0-kernelsu-arm64.zip`, reboot, and verify daemon/watchdog
   readiness plus persistence under `/data/adb/xiao`.
2. Configure a real Telegram bot and authorized Chat ID; verify an unauthorized
   principal creates no session/message/memory/skill/run mutation.
3. Exercise `/session`, `/btw`, concurrent switching, `/stop`, and `/retry`;
   inspect MAIN/SIDE rows and durable run statuses after restart.
4. Complete Codex and Antigravity OAuth with authorized accounts and execute a
   real Codex tool continuation (`context_stats` or memory search).
5. Verify explicit remember, preference change, and forget across daemon
   restart, including one canonical active memory row and separate history.
6. Create enough history to trigger a summary; confirm raw message count is
   unchanged and `session_search` finds old principal-owned content.
7. Complete a meaningful verified reusable workflow twice with an improvement;
   confirm one canonical skill is updated and failed/cancelled work creates no
   skill.
8. Verify managed Termux wrappers, authenticated loopback IPC, WebUI restart,
   bounded/redacted logs, and module update/uninstall preservation.

Real provider credentials, Telegram delivery, Android lifecycle, and device
packaging are not available to host unit tests and must remain reported as
manual/device validation.
