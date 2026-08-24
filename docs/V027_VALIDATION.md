# Xiao v0.2.7 Validation Record

GitHub Actions is the authoritative Rust/Android validation environment for this release candidate.
Authoritative release validation is the latest successful required CI run whose `head_sha` exactly equals the release candidate commit.

This document is updated from the actual hardening worktree. A local green host
run is evidence for the committed source only after the final SHA is recorded;
it does not substitute for the required exact-head GitHub Actions Android job.

## Authoritative Validation Methodology

Release validation requires both `rust` and `android-arm64` jobs in `.github/workflows/ci.yml` to succeed on the exact commit without waivers or skipped checks.

Workflow configuration:
- Triggers: `pull_request`, `push: branches: [main]`, `workflow_dispatch`
- Permissions: `contents: read`
- Pinned SHA actions (`actions/checkout@3d3c4`, `dtolnay/rust-toolchain@4360b5`, `actions/upload-artifact@043fb4`)
- No `pull_request_target`, no secret consumption, no self-push.

## Validation Gates (Executed in CI)

All gates run on `ubuntu-24.04` with `Rust 1.98.0`, `CARGO_TERM_COLOR=always`, `CARGO_INCREMENTAL=0`.

| # | Gate | Exact Command | Required Outcome |
|---|---|---|---|
| 1 | POSIX shell syntax | `shellcheck -x -s sh module/*.sh module/termux/xiao-wrapper scripts/device-custom-e2e.sh` | PASS (exit 0, no findings) |
| 2 | Bash shell syntax | `shellcheck -s bash packaging/build-module.sh scripts/acceptance.sh` | PASS |
| 3 | Rust formatting | `cargo fmt --all -- --check` | PASS (no diff) |
| 4 | Type check | `cargo check --locked --all-targets --all-features` | PASS |
| 5 | Tests | `cargo test --locked --all-targets --all-features` | PASS (all tests pass, 0 failed) |
| 6 | Lints | `cargo clippy --locked --all-targets --all-features -- -D warnings` | PASS (0 warnings) |
| 7 | Release build (host) | `cargo build --locked --release --all-features` | PASS |
| 8 | WebUI syntax app.js | `node --check module/webroot/assets/app.js` | PASS |
| 9 | WebUI syntax ksu-bridge.js | `node --check module/webroot/assets/ksu-bridge.js` | PASS |
| 10 | Static acceptance | `./scripts/acceptance.sh --static-only` | PASS (60+ static invariants) |
| 11 | Android arm64 cross-compile | `cargo ndk -t arm64-v8a build --locked --release --bin xiaod --bin xiao` | **OPEN/BLOCKED locally** — Android NDK is not installed in this environment; CI exact-head gate remains required |
| 12 | Deterministic module ZIP | `packaging/build-module.sh` (twice) → `sha256 equality`, `sha256sum -c dist/*.sha256`, `unzip -t` | **OPEN/BLOCKED locally** — packaging is intentionally GitHub-Actions-only and requires the arm64 build outputs |
| 13 | Whitespace hygiene | `git diff --check` | PASS |

Historical pre-release gate (informational, not the v0.2.7 artifact):
candidate `d6f8edd7f56efc472fca8fac7d493a4026e26ddd` passed run #134 (`32686544237`) on the same toolchain with
the full matrix; produced `xiao-v0.2.6-kernelsu-arm64` (artifact `9506162200`) as evidence-only.

## P0 / P1 / P2 status

Source of truth for priorities: the mandatory scenario matrix in `docs/ACCEPTANCE.md`
and the final-hardening architecture package. Host-verifiable criteria below are
covered by deterministic tests; exact-head CI, rooted-device, and live-network
checks remain open where this environment cannot exercise them.

### P0 — Release-blocking (control-plane & safety)

| Item | Scope | Host evidence | Status |
|------|-------|---------------|--------|
| P0-1 | Stable installation owner and replaceable Telegram binding; ambiguous legacy owners fail closed | `stable_owner_state_is_global_while_dm_group_and_topics_stay_isolated`, `installation_owner_has_no_telegram_identity_semantics`, `representative_v025_state_migrates_transactionally_and_idempotently`, `multiple_legacy_owner_rows_fail_closed_until_explicit_telegram_resolution` | **PASS (host)** |
| P0-2 | Shared Telegram setup service (write-only/versioned token, atomic owner binding, probe-before-commit, authoritative SQLite) | `telegram_setup_config_snapshot_failure_commits_authoritative_state_with_warning`, `telegram_probe_failure_keeps_old_token_binding_and_control_state_active`, `telegram_late_db_failure_rolls_back_binding_and_staged_token_as_one_transaction`, `telegram_post_commit_secret_cleanup_failure_is_success_with_warning` | **PASS (host)** |
| P0-3 | Structured CLI command tree & stable JSON/error/exit semantics (unknown → usage exit 2, never chat) | `tests/cli_integration.rs` (root help snapshot, typo→2, aliases→2, JSON envelope, subcommand help) | **PASS (host)** |
| P0-4 | Explicit session targeting (CLI sessions independent unless `--session ID`; no cross-leak) | `command/mod.rs` session tests, `session/mod.rs` cross-principal rejection | **PASS** |
| P0-5 | Exact one-shot durable approval binding (owner/session/run/call/tool/args hash) | `approval_is_exact_one_shot_and_cannot_cross_sessions_or_runs`, `privileged_tool_requires_exact_durable_one_shot_approval` | **PASS** |
| P0-6 | No unrestricted root shell / no `Command::new("sh|bash|su")` path | `acceptance.sh` rejects `Command::new("sh"...` + `RootShell` sentinel | **PASS** |
| P0-7 | No MCP / subagents / vector DB / cron / native plugins | `acceptance.sh` sentinel `No MCP…` | **PASS** |
| P0-8 | Exact-model probe lifecycle; Unprobed/Indeterminate agent protocol cannot activate | `unprobed_custom_model_is_explicitly_chat_only`, `custom_model_readiness_semantics_handles_optional_vision_and_file_capabilities` | **PASS (host)** |

### P1 — Control-plane parity (feature completeness)

| Item | Scope | Host evidence | Status |
|------|-------|---------------|--------|
| P1-1 | WebUI Telegram setup + exact-session AI configuration (typed `xiaod` admin actions only) | `webui_uses_only_typed_xiaod_manager_actions`, `manager_*` tests | **PASS** |
| P1-2 | Tri-state Custom tools / structured / continuation / vision / file capabilities (cached probe, non-destructive doctor) | `probe_custom_tool_capability`, `codex_antigravity_and_custom_protocols_keep_the_same_agent_tool_workflow`, `production_custom_structured_fallback_retains_tool_a_and_b_results_until_final`, `unprobed_custom_model_is_explicitly_chat_only` | **PASS (host)** |
| P1-3 | CLI file/image chat (`xiao chat --file/--image`) + session scoping | `attachments::tests`, `bin_cli.rs` ingestion paths, `telegram_photo_and_document_are_downloaded_scoped_and_indexed` | **PASS** |
| P1-4 | Scanned-PDF planner: embedded text, bounded OCR, verified provider file/vision fallback, explicit blocked state | `wrong_txt_extension_cannot_override_pdf_magic_and_empty_pdf_requires_ocr`, `scanned_pdf_provider_file_input_path_is_durable_and_real`, `scanned_pdf_provider_vision_renders_pages_before_calling_provider`, `agent_engine_runs_scanned_pdf_provider_file_fallback_before_final_answer`, `scanned_pdf_unknown_or_unsupported_capabilities_are_blocked_explicitly` | **PASS (host)** |
| P1-5 | Attachment quota / retention / orphan / active-run protection with atomic session/owner/global reservations | `concurrent_quota_reservations_cannot_exceed_session_quota`, `concurrent_quota_reservations_cannot_exceed_owner_or_global_quota`, `quota_reservation_release_and_orphan_cleanup_are_durable`, `active_run_protects_attachment_from_manual_and_retention_cleanup` | **PASS (host)** |
| P1-6 | Full Custom profile editing/deletion with credential/header safety and post-commit cleanup warnings | `endpoint_edit_clears_credentials_and_headers`, `endpoint_replacement_swaps_all_profile_scoped_secrets_in_one_patch`, `custom_profile_a_secrets_never_reach_profile_b`, `existing_credential_ref_must_be_same_owner_and_custom_provider`, `profile_db_failure_after_secret_staging_leaves_old_state_and_no_new_refs`, `post_commit_secret_gc_failure_is_success_with_bounded_warning`, `profile_delete_commits_then_collects_versioned_secret_and_credential` | **PASS (host)** |
| P1-7 | Bounded live-or-CACHED Doctor probes | `doctor_reports_memory_failure_independently_from_healthy_database`, doctor/manager tests | **PASS** |
| P1-8 | Telegram/CLI/WebUI parity (provider, memory, skill, approval, diagnostics, session AI) | `acceptance.sh` v0.2.7 surfaces check, `telegram/scope.rs` + `telegram/commands.rs` tests | **PASS** |
| P1-9 | CLI DTO hygiene & alias collision fixes | `clippy -D warnings` PASS, `root_help_matches_snapshot` snapshot stable | **PASS** |
| P1-10 | Parent cancellation through attachment/OCR/provider/tool work | `scanned_pdf_provider_fallback_honors_run_cancellation`, `cancellation_during_tool_marks_both_run_boundaries_terminal`, Telegram attachment cancellation path | **PASS (host)** |
| P1-11 | Append-oriented live timeline, safe budget, exact correlation, redacted failures, and final stripping | `normal_timeline_retains_24_append_oriented_rows`, `detailed_timeline_retains_30_append_oriented_rows`, `correlation_id_completes_exact_tool_row_and_rejects_wrong_id`, `failed_tool_stays_visible_with_redacted_error_and_failure_icon`, `hard_progress_budget_preserves_active_and_recent_rows`, `completed_tool_remains_visible_without_synthetic_thinking`, `stream_progress_updates_one_writing_step_in_place`, `final_surface_excludes_progress_and_keeps_side_marker` | **PASS (host)** |
| P1-12 | Semantic ProgressIcon/TelegramEmojiRegistry with validated custom-ID fallback and policy isolation | `invalid_custom_emoji_id_falls_back_to_unicode_without_broken_draft`, `active_progress_uses_the_official_ai_actions_emoji`, `action_classifier_is_presentation_only_and_does_not_relax_policy` | **PASS (host)** |

### P2 — Polish & robustness

| Item | Scope | Host evidence | Status |
|------|-------|---------------|--------|
| P2-1 | Skills pagination 13→5/5/3 with bounded selection | `skills_manager_paginates_thirteen_entries_as_five_five_three` | **PASS** |
| P2-2 | Wizard Back/pagination index & vision nonce fragment fix | commit `40d41dd` + `1cbc7df` covered by `discovery_failure_exposes_concrete_recovery_actions`, `custom_wizard_retry_and_back_are_phase_aware_and_replace_transient_keys` | **PASS** |
| P2-3 | Deterministic packaging & checksum sidecar | Required CI packaging job; local environment has no Android NDK and packaging guard is GitHub-Actions-only | **OPEN/BLOCKED locally** |
| P2-4 | Shell/JS/TOML hygiene | ShellCheck ×2 + `node --check` ×2 + `TOML parses` PASS | **PASS** |

All host-verifiable P0/P1/P2 checks are green. Android arm64, deterministic
module packaging, exact-head CI, and real-device/live-provider checks are not
claimed green here.

## Remaining real-device checks (not claimed)

Host tests use fake runtime/transport/executor/provider/attachment/Android boundaries and do not impersonate
rooted Android, live Telegram, or live provider credentials. The following must be validated on a rooted arm64
device with live accounts before marking the PR ready for production rollout:

1. Flash `xiao-v0.2.7-kernelsu-arm64.zip` on a rooted device, reboot; verify `/data/adb/xiao` persistence,
   `post-fs-data` + `service.sh` + `watchdog.sh` readiness, SELinux, and that root `xiaod` drops to the
   real Termux app UID/GID for general commands.
2. Detect Termux PATH/home/package manager across installations; auto-install a missing trusted package from
   the configured Termux repository and cancel one in-progress install.
3. Live Telegram bot/owner ACL against DM/group/topics; new-prompt Custom wizard error recovery,
   exact approval/retry, `/cancel`, YOLO, memory/skills/model managers, photo/PDF/DOCX ingestion, vision,
   and `sendDocument` result upload.
4. Real Codex and Antigravity browser OAuth completion, refresh, and verified tool continuation (no provider
   receives tools it cannot continue).
5. Typed `AndroidBroker` inspection/restart under KernelSU/Magisk with exact one-shot approval.
6. Manual `USER.md` / `MEMORY.md` / `SKILL.md` edits, restart, reconciliation/history without `SOUL.md` replacement.
7. Full KernelSU WebUI exercise through the real admin bridge (all eleven sections, cancellation, masked provider
   management, diagnostics rerun, log export).

These seven items intentionally have no host impersonation.

## Final report format (per prompt)

The release PR description and this file together constitute the final validation report. Each future green head
MUST include:

```text
Head: <full SHA>
Run:  <actions run URL>  +  <run id>
Jobs: rust <job URL> success | android-arm64 <job URL> success
Gates table: # | Gate | Exact command (as in ci.yml) | PASS/FAIL + evidence line
Counts: cargo test <N> passed, 0 failed (split per test binary)
Artifact: xiao-v0.2.7-kernelsu-arm64.zip + .sha256 — byte-identical dual build, SHA verified, ZIP integrity ok
P0/P1/P2 table: item | scope | host evidence | PASS
Real-device: 7 checklist items — OPEN / PASS with device evidence where completed
```

No CI run or Android/package artifact is currently available for this candidate head.
Those fields must be filled with evidence for the exact final SHA before release is
declared.

`cargo fmt --all -- --check` is a hard gate: any formatting diff is a FAIL.

## Workflow / governance audit

The final CI workflow uses `pull_request` plus `workflow_dispatch`, `contents: read`, `persist-credentials: false`,
and full-SHA pinned reusable actions. It has no `pull_request_target`, secret consumption, self-push, or workflow
mutation path. Source governance includes `CODEOWNERS` and `SECURITY` policy. Live repository metadata reports
`main` unprotected with no required checks; the available connector exposes no branch-protection/ruleset mutation,
so that external posture is recorded rather than falsely claimed fixed.

## CLI snapshots

`tests/snapshots/cli_help_body.txt` is the frozen expected body for `xiao --help` (version line excluded).
`tests/cli_integration.rs` enforces:

- `root_help_matches_snapshot` — stdout equals snapshot exactly
- `typo_is_usage_error_and_never_falls_through_to_chat` — exit 2, stderr hints `did you mean`
- `removed_aliases_remain_usage_errors` — `about`/`logout` are exit 2
- `json_usage_error_has_stable_application_envelope` — `{"status":"error","error":{"code":"unknown_command"}}` with no legacy `ok/view/actions/buttons` keys
- `subcommand_help_is_terminal_native_and_does_not_require_daemon`

Snapshot was valid on `32701638246`; any drift must update both code and snapshot together.

## Environment limitation

No rooted Android device, live Telegram account, or live provider credentials are available here.
Physical-device/live-network acceptance is therefore unclaimed; deterministic fakes/regressions plus
Android arm64 cross-build and deterministic packaging are used where supported.

The exact v0.2.7 release head must pass the same full CI matrix before the PR is marked ready for review.
