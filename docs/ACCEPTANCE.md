# Xiao v0.2.0 Final-Architecture Acceptance Coverage

This maps the final v0.2.0 acceptance criteria to implementation and
deterministic tests. Xiao is a private single-owner agent; `principal` remains
only as a compatibility/session isolation key. Host tests use fake runtime and
executor boundaries and do not claim real rooted-Android validation.

## Mandatory scenario matrix

| Scenario | Automated coverage | Result required |
|---|---|---|
| Identity survives restart | `identity_bootstrap_survives_restart_and_never_overwrites_owner_files`; `runtime_context_contains_persistent_identity_owner_and_verified_environment` | Living files survive bootstrap and enter a new context; SOUL is not overwritten |
| Preference replacement | `synonymous_explicit_preference_change_updates_one_canonical_memory_and_file`; `generalized_preferences_facts_and_manual_edits_reconcile` | One active semantic preference, updated USER.md, SQLite audit retained |
| Missing Termux dependency | `trusted_missing_dependency_is_installed_reprobed_and_audited`; `missing_dependency_installs_reprobes_and_resumes_original_command` | Trusted package installs, binary is re-probed, original command resumes, progress/audit exist |
| First approach fails | `failure_changes_strategy_and_unverified_final_continues_until_evidence` | Failure is observed, arguments change, unverified final continues, later evidence completes |
| Action requires verification | `action_claim_without_evidence_is_not_yet_verified` | Bare “done,” action-only, and same-call verification labels remain `NotYetVerified` |
| Successful task learns | `failure_changes_strategy_and_unverified_final_continues_until_evidence`; `observable_trace_creates_generalized_skill_with_pitfall_then_updates_same_skill` | Verified reusable trace creates a generalized skill containing recovered pitfalls |
| Similar task updates skill | `verified_work_creates_then_updates_one_canonical_skill`; `related_skill_updates_canonical_row_instead_of_creating_duplicate` | Related intent updates one canonical skill and history, not a duplicate |
| Privileged policy | `broker_surface_is_typed_and_restart_is_approval_classed`; `privileged_tool_requires_exact_durable_one_shot_approval` | Typed broker only; exact approval is consumed once; no root-shell tool |
| No false capability refusal | `capability_resolution_prevents_false_cannot_when_termux_backend_is_usable`; `capability_resolution_distinguishes_available_installable_and_approval` | Termux aliases resolve available and have no blocker; missing trusted binary is installable |
| Telegram progress/cancel/files | `long_generation_does_not_block_stop_other_principal_or_callbacks`; semantic progress tests; `result_file_is_sent_through_telegram_multipart_document_path`; command approval test | `/stop` remains responsive, progress stays semantic/redacted, document uses multipart, approval commands work |

## Architecture coverage

### Identity, environment, and capabilities

- `IdentityWorkspace` create-loads `SOUL.md`, `USER.md`, `MEMORY.md`,
  `AGENTS.md`, and `ENVIRONMENT.md` with private permissions and atomic writes.
- `RuntimeState` refreshes only generated `ENVIRONMENT.md`.
- `EnvironmentProbe` is fakeable and captures platform/Android, architecture,
  Xiao version, UID, root evidence, SELinux, Termux prefix/home/PATH/shell,
  package manager, selected binaries, and execution backends.
- `CapabilityRegistry` represents available, missing-installable,
  approval-required, temporary, unsupported, forbidden, and unknown states.
- Coverage: all `identity::tests`, `runtime::environment::tests`, and
  `runtime::capabilities::tests`.

### Provider-agnostic tools and execution

- Canonical `ToolSpec` includes origin, effect, risk, capability requirements,
  schema, and timeout. Providers only translate specs.
- `ToolRegistry` dispatches canonical tools/aliases, gates capabilities,
  enforces runtime `ToolPolicy`, records approval status through the agent
  audit path, bounds output, and redacts results.
- `TermuxExecutor` uses structured argv, Termux-only PATH, controlled cwd/env,
  root-daemon UID/GID/supplementary-group drop plus `no_new_privs`, timeout, cancellation, and
  drained-but-bounded output. It rejects root escalation, `-c` shell strings,
  unmanaged package mutation, and remote installer pipelines.
- Argument-aware policy requires exact approval for destructive commands,
  opaque shell scripts, and credential-sensitive access.
- `DependencyResolver` accepts only trusted normalized package mappings,
  records installs, re-probes, and then resumes.
- `AndroidBroker` accepts typed Xiao-service operations only; restart requires
  approval and no model-controlled command string.
- Coverage: `provider_translates_canonical_tool_specs_without_owning_policy`,
  all `tools::registry::tests`, `tools::policy::tests`,
  `runtime::execution::tests`, `runtime::dependency::tests`, and
  `runtime::android::tests`.

### Memory, recall, context, and skills

- USER/MEMORY files are active current state; stable managed entries support
  none/create/update/delete/merge/rekey and reconcile manual edits. SQLite
  remains index/history, including a legacy SQLite-to-file bridge.
- `messages_fts` provides owner-filtered old-session recall. Context combines
  hard rules, SOUL, verified runtime/capabilities, USER, relevant MEMORY,
  AGENTS, selected skill bodies, summaries, FTS excerpts, recent turns, and
  current request under a character budget.
- Filesystem `skills/<name>/SKILL.md` accepts YAML `name`/`description`, common
  optional metadata, namespaced Xiao requirements, discovery/reconciliation,
  lazy view, eligibility gating, and safe dependency resolution.
- Coverage: all `memory::evaluator::tests`, `memory::store::tests`,
  `context::retrieval::tests`, `context::engine::tests`,
  `skills::filesystem::tests`, and `skills::store::tests`.

### Agent loop, verification, learning, and Telegram

- Configured turn/tool/no-progress/runtime limits prevent unbounded loops.
  Cancellation is checked around provider/tool work; identical failed action
  signatures are rejected.
- Completion states are `VerifiedSuccess`, `NotYetVerified`, `Blocked`, and
  `Failed`. `NotYetVerified` becomes a new provider observation and continues.
- Learning runs only after verified success from the bounded observable trace.
  It generalizes prerequisites/procedure/pitfalls/verification and searches for
  related skills before creating; failed/cancelled/unverified/trivial traces do
  not produce positive skills.
- Telegram exposes semantic progress, trusted-install progress, `/approvals`,
  `/approve`, `/deny`, `/stop`, blockers/finals, and bounded verified document
  results. Hidden reasoning has no event or persistence field.
- Coverage: `agent::completion::tests`, agent adaptive/cancel/bounds tests,
  `learning::evaluator::tests`, `owner_can_inspect_approve_and_deny_pending_operations`,
  Telegram progress/cancellation tests, and multipart document test.

### Storage, migration, recovery, and exclusions

- Migration version 10 adds `approvals`, `dependency_installs`,
  `environment_probes`, `workspace_file_index`, and `skill_file_index` after
  the v6-v9 run/memory/FTS/skill migrations.
- `v020_migration_is_fresh_and_idempotent_with_consistent_fts` expects version
  10 and every new object; `v010_database_upgrades_additively_without_losing_history`
  preserves legacy history; reopen tests quarantine uncertain agent/tool and
  package-install work rather than replaying it.
- Static acceptance rejects an unrestricted model root-shell path and checks
  absence of MCP, subagents, vector DB, cron missions, and native plugins.

## Quality gates

Run from the repository root:

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets --all-features
cargo test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
./scripts/acceptance.sh --static-only
git diff --check
```

The final implementation report must state actual outcomes rather than treating
this checklist as evidence that a command ran.

## Real-device validation still required

1. Flash the arm64 module on a rooted Android device and reboot; verify private
   data/workspace persistence, watchdog readiness, SELinux behavior, and that
   root `xiaod` drops general commands to the real Termux app identity.
2. Confirm detected Termux PATH/home/package manager across supported Termux
   installations; auto-install a missing trusted package from the configured
   Termux repository and cancel one install in progress.
3. Configure a real Telegram bot/owner ACL; exercise semantic progress,
   approval/retry, `/stop`, dependency progress, blocker delivery, and document
   upload.
4. Complete real Codex/Antigravity OAuth and tool continuation using owner
   accounts; confirm no provider receives tools it cannot continue.
5. Exercise the typed Xiao-service inspection/restart broker under the actual
   KernelSU/Magisk service model and verify exact one-shot approval.
6. Manually edit USER.md, MEMORY.md, and a community SKILL.md, restart, and
   verify reconciliation/history without SOUL replacement.

Real credentials, Telegram delivery, Android init/root/SELinux behavior, and
device package repositories are intentionally not impersonated by host tests.
