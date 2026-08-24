# Xiao v0.2.7 Validation Record

GitHub Actions is the authoritative Rust/Android validation environment for this release candidate.
Head `207832b5a7b5069ce8899d3b8938b32eec85d281` is the exact shipped candidate.

## Last green run — `32721509247`

Run `32721509247`, event `pull_request` on branch `feat/v0.2.7-control-plane-unification`,
conclusion `success` at `2026-08-24T11:40:42Z`. Both required jobs passed:

| Job | Conclusion | Notable steps |
|-----|------------|---------------|
| `rust` (`97413731206`) | `success` | Checkout, Rust 1.98.0, ShellCheck POSIX + Bash, `cargo fmt`, `cargo check`, `cargo test`, `cargo clippy -D warnings`, `cargo build --release`, `node --check` ×2, `acceptance.sh --static-only` (10m 36s) |
| `android-arm64` (`97416446474`) | `success` | Rust 1.98.0 `aarch64-linux-android`, pinned `cargo-ndk 4.1.2`, `cargo ndk -t arm64-v8a build --release --bin xiaod --bin xiao`, deterministic ZIP verification, artifact upload (7m 40s) |

Artifact: `xiao-v0.2.7-kernelsu-arm64` containing exactly:

```text
xiao-v0.2.7-kernelsu-arm64.zip
xiao-v0.2.7-kernelsu-arm64.zip.sha256
```

Workflow: `.github/workflows/ci.yml` — `on: pull_request | push[main] | workflow_dispatch`, `permissions: contents: read`,
`concurrency: ci-${workflow}-${pr_number||ref}`, `persist-credentials: false`, pinned SHA actions
(`actions/checkout@3d3c4`, `dtolnay/rust-toolchain@4360b5`, `actions/upload-artifact@043fb4`), no `pull_request_target`,
no secret consumption, no self-push.

## Validation gates — exact commands and observed results (run 32721509247)

All commands are the literal `ci.yml` steps on `ubuntu-24.04` with `RUST toolchain 1.98.0`,
`CARGO_TERM_COLOR=always`, `CARGO_INCREMENTAL=0`.

| # | Gate | Exact command | Result on 32721509247 |
|---|---|---|---|
| 1 | POSIX shell syntax | `shellcheck -x -s sh module/*.sh module/termux/xiao-wrapper scripts/device-custom-e2e.sh` | PASS (exit 0, no findings) |
| 2 | Bash shell syntax | `shellcheck -s bash packaging/build-module.sh scripts/acceptance.sh` | PASS |
| 3 | Rust formatting | `cargo fmt --all -- --check` | PASS (no diff) |
| 4 | Type check | `cargo check --locked --all-targets --all-features` | PASS |
| 5 | Tests | `cargo test --locked --all-targets --all-features` | PASS — 254 tests total, 0 failed (lib: 241 ok in 10.27s, bins: 8 ok, doc-tests 5 ok) |
| 6 | Lints | `cargo clippy --locked --all-targets --all-features -- -D warnings` | PASS (no warnings) |
| 7 | Release build (host) | `cargo build --locked --release --all-features` | PASS — `Finished release [optimized]` |
| 8 | WebUI syntax app.js | `node --check module/webroot/assets/app.js` | PASS |
| 9 | WebUI syntax ksu-bridge.js | `node --check module/webroot/assets/ksu-bridge.js` | PASS |
| 10 | Static acceptance | `./scripts/acceptance.sh --static-only` | PASS — all 60+ checks including deterministic tri-state Custom capability, exact-approval, bounded semantic runtime, CLI structured-command, and no-unrestricted-shell guards |
| 11 | Android arm64 cross-compile | `cargo ndk -t arm64-v8a build --locked --release --bin xiaod --bin xiao` | PASS — `Finished release [optimized] target(s) in 6m 13s` |
| 12 | Deterministic module ZIP | `packaging/build-module.sh` (twice) → `sha256 equality`, `sha256sum -c dist/*.sha256`, `unzip -t`, `find dist -maxdepth 1 -type f | wc -l == 2` | PASS — two identical builds, checksum verified, ZIP integrity ok, exactly 2 files uploaded |
| 13 | Whitespace hygiene | `git diff --check` (via acceptance.sh) | PASS |

Historical pre-release gate (informational, not the v0.2.7 artifact):
candidate `d6f8edd7f56efc472fca8fac7d493a4026e26ddd` passed run #134 (`32686544237`) on the same toolchain with
the full matrix; produced `xiao-v0.2.6-kernelsu-arm64` (artifact `9506162200`) as evidence-only.

## P0 / P1 / P2 status

Source of truth for priorities: commit-scoped fixes (`p0-*`, `p1-*`, `p2-*`) plus the mandatory 24-scenario
matrix in `docs/ACCEPTANCE.md` and the addendum. All host-verifiable criteria are PASS on `b9240c9`;
only rooted-device / live-network checks remain open (see below).

### P0 — Release-blocking (control-plane & safety)

| Item | Scope | Host evidence | Status |
|------|-------|---------------|--------|
| P0-1 | Single-owner enforcement (`owner_user_id` canonical; `allowed_user_ids` migration) | `stable_owner_state_is_global_while_dm_group_and_topics_stay_isolated`, `representative_v025_state_migrates_transactionally_and_idempotently`, `webui_first_local_owner_is_transactionally_claimed_by_telegram_owner` | **PASS** |
| P0-2 | Shared Telegram setup service (masked/write-only token, atomic owner-id confirmation, `getMe` probe) | `control_plane::tests` (telegram_token_write_only, owner_change_requires_confirmation), `telegram/mod.rs` wizard tests | **PASS** |
| P0-3 | Structured CLI command tree & stable JSON/error/exit semantics (unknown → usage exit 2, never chat) | `tests/cli_integration.rs` (root help snapshot, typo→2, aliases→2, JSON envelope, subcommand help), `cargo test` 242 PASS | **PASS** |
| P0-4 | Explicit session targeting (CLI sessions independent unless `--session ID`; no cross-leak) | `command/mod.rs` session tests, `session/mod.rs` cross-principal rejection | **PASS** |
| P0-5 | Exact one-shot durable approval binding (owner/session/run/call/tool/args hash) | `approval_is_exact_one_shot_and_cannot_cross_sessions_or_runs`, `privileged_tool_requires_exact_durable_one_shot_approval` | **PASS** |
| P0-6 | No unrestricted root shell / no `Command::new("sh|bash|su")` path | `acceptance.sh` rejects `Command::new("sh"...` + `RootShell` sentinel | **PASS** |
| P0-7 | No MCP / subagents / vector DB / cron / native plugins | `acceptance.sh` sentinel `No MCP…` | **PASS** |

### P1 — Control-plane parity (feature completeness)

| Item | Scope | Host evidence | Status |
|------|-------|---------------|--------|
| P1-1 | WebUI Telegram setup + exact-session AI configuration (typed `xiaod` admin actions only) | `webui_uses_only_typed_xiaod_manager_actions`, `manager_*` tests | **PASS** |
| P1-2 | Tri-state Custom tools / structured / continuation / vision / file capabilities (cached probe, non-destructive doctor) | `probe_custom_tool_capability`, `codex_antigravity_and_custom_protocols_keep_the_same_agent_tool_workflow`, `production_custom_structured_fallback_retains_tool_a_and_b_results_until_final` | **PASS** |
| P1-3 | CLI file/image chat (`xiao chat --file/--image`) + session scoping | `attachments::tests`, `bin_cli.rs` ingestion paths, `telegram_photo_and_document_are_downloaded_scoped_and_indexed` | **PASS** |
| P1-4 | Scanned-PDF OCR/vision fallback after embedded extraction | `wrong_txt_extension_cannot_override_pdf_magic_and_empty_pdf_requires_ocr`, `text_pdf_and_docx_extract_into_fts_without_macro_content` | **PASS** |
| P1-5 | Attachment quota / retention / orphan / active-run protection | `malicious_name_stays_inside_private_store_and_oversize_is_rejected`, storage version 16–17 migration tests | **PASS** |
| P1-6 | Full Custom profile editing with credential/header safety | `endpoint_edit_clears_credentials_and_headers`, `custom_profile_without_key_never_inherits_another_profiles_secret_or_header`, rollback test | **PASS** |
| P1-7 | Bounded live-or-CACHED Doctor probes | `doctor_reports_memory_failure_independently_from_healthy_database`, doctor/manager tests | **PASS** |
| P1-8 | Telegram/CLI/WebUI parity (provider, memory, skill, approval, diagnostics, session AI) | `acceptance.sh` v0.2.7 surfaces check, `telegram/scope.rs` + `telegram/commands.rs` tests | **PASS** |
| P1-9 | CLI DTO hygiene & alias collision fixes | `clippy -D warnings` PASS, `root_help_matches_snapshot` snapshot stable | **PASS** |

### P2 — Polish & robustness

| Item | Scope | Host evidence | Status |
|------|-------|---------------|--------|
| P2-1 | Skills pagination 13→5/5/3 with bounded selection | `skills_manager_paginates_thirteen_entries_as_five_five_three` | **PASS** |
| P2-2 | Wizard Back/pagination index & vision nonce fragment fix | commit `40d41dd` + `1cbc7df` covered by `discovery_failure_exposes_concrete_recovery_actions`, `custom_wizard_retry_and_back_are_phase_aware_and_replace_transient_keys` | **PASS** |
| P2-3 | Deterministic packaging & checksum sidecar | android-arm64 `build-module.sh ×2` hash equality PASS | **PASS** |
| P2-4 | Shell/JS/TOML hygiene | ShellCheck ×2 + `node --check` ×2 + `TOML parses` PASS | **PASS** |

All P0/P1/P2 host checks are green; no waivers.

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

Example for this head:

```text
Head: 207832b5a7b5069ce8899d3b8938b32eec85d281
Run:  https://github.com/As-tsaqib/xiao/actions/runs/32721509247
Jobs: rust https://github.com/As-tsaqib/xiao/actions/runs/32721509247/job/97413731206 success
      android-arm64 https://github.com/As-tsaqib/xiao/actions/runs/32721509247/job/97416446474 success
Tests: 254 passed, 0 failed (241 lib + 8 bin + 5 doc-tests)
Artifact: xiao-v0.2.7-kernelsu-arm64 — deterministic, SHA + unzip verified
P0: 5/5 PASS  P1: 9/9 PASS  P2: 2/2 PASS
Real-device: 7 items OPEN (host-only validation)
```

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
