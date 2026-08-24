# Xiao v0.2.7 Control-Plane Unification, Reliability, Multimodal, and Management Acceptance Coverage

This maps retained v0.2.6 regressions plus final v0.2.7 acceptance criteria to implementation and
deterministic tests. Xiao is a private single-owner agent; stable
`OwnerIdentity` owns global durable state while `TelegramScope` and
`XiaoSession` isolate conversation state. Legacy principal values remain only
as migration compatibility keys. Host tests use fake runtime, transport,
executor, provider, attachment, and Android boundaries and do not claim real
rooted-Android validation.

## Mandatory scenario matrix

| # | Scenario | Automated coverage | Required result |
|---:|---|---|---|
| 1 | Stable owner across scopes | `stable_owner_state_is_global_while_dm_group_and_topics_stay_isolated`; `telegram_owner_is_independent_of_chat_and_topic` | One owner-global memory/skills/profile namespace; DM/group/topic sessions remain isolated |
| 2 | Custom credential leakage | `custom_profile_without_key_never_inherits_another_profiles_secret_or_header` | Profile B sends neither Authorization nor profile A secret/header |
| 3 | Endpoint credential safety | `endpoint_edit_clears_credentials_and_headers` | Endpoint change clears profile-bound credential/header state |
| 4 | Structured fallback multi-turn | `production_custom_structured_fallback_retains_tool_a_and_b_results_until_final`; strict fallback tests | Production request 2 includes result A; request 3 includes results A+B; undeclared calls cannot execute |
| 5 | Approval isolation | `approval_is_exact_one_shot_and_cannot_cross_sessions_or_runs`; `privileged_tool_requires_exact_durable_one_shot_approval` | Owner/session/run/call/tool/argument binding is one-shot and expiring |
| 6 | YOLO | `yolo_converts_only_ask_to_audited_allow_and_never_bypasses_deny`; topic YOLO tests | ASK auto-grant is session-local/audited; DENY remains denied |
| 7 | Wizard progression | Telegram Custom wizard E2E; scoped/expiry/menu tests; `credential_input_payload_is_scrubbed_without_changing_inbox_state` | Each phase uses a new prompt, old keyboard is retired, stale input is rejected, and credential payload is scrubbed |
| 8 | Wizard error recovery | `discovery_failure_exposes_concrete_recovery_actions`; `custom_wizard_retry_and_back_are_phase_aware_and_replace_transient_keys`; rollback test | Concrete error plus working phase-aware Retry/Edit Endpoint/Back/Close without orphan credentials or partial profiles |
| 9 | Vision-capable path | `production_custom_vision_serializes_normalized_image_and_rejects_nonvision_model`; Telegram ingestion test | Normalized validated image and caption reach a verified vision model |
| 10 | Non-vision path | same production vision test; capability-gating tests | Image bytes are withheld and a factual model-switch/capability blocker is returned |
| 11 | PDF text | `text_pdf_and_docx_extract_into_fts_without_macro_content`; document retrieval test | Embedded PDF text is chunked, indexed, and retrieved relevantly |
| 12 | Image-only PDF | `wrong_txt_extension_cannot_override_pdf_magic_and_empty_pdf_requires_ocr` | Empty embedded text becomes `needs_ocr`, never false success |
| 13 | DOCX safety | `text_pdf_and_docx_extract_into_fts_without_macro_content` | Text extracts while macro/script parts are ignored |
| 14 | Wrong extension | `wrong_txt_extension_cannot_override_pdf_magic_and_empty_pdf_requires_ocr` | Sniffed PDF type overrides `.txt` name |
| 15 | Oversize/path safety | `malicious_name_stays_inside_private_store_and_oversize_is_rejected` | Limits hold and names cannot escape the private attachment root |
| 16 | Memory reconciliation | `managed_entries_replace_duplicates_and_manual_edits_are_reindexed`; memory manager tests | Manual USER/MEMORY edits appear after manager reconciliation |
| 17 | Skills pagination | `skills_manager_paginates_thirteen_entries_as_five_five_three` | Thirteen skills render 5/5/3 with bounded selection |
| 18 | Doctor truthfulness | `doctor_reports_memory_failure_independently_from_healthy_database` | DB can PASS while Memory independently WARN/FAIL |
| 19 | Semantic runtime | `provider_evaluations_use_one_reusable_runtime_with_bounded_concurrency`; cancellation test | No per-call runtime/thread; concurrency, timeout, and cancellation are bounded |
| 20 | Command registry | `public_registry_is_exact_and_hidden_commands_are_not_advertised`; help/setMyCommands equality tests | Exactly 17 public commands; `/about` and `/logout` absent |
| 21 | Model disconnect isolation | `model_disconnect_removes_only_selected_account_and_detaches_its_sessions` | Selected account/profile is removed without damaging unrelated state |
| 22 | WebUI single writer | `webui_uses_only_typed_xiaod_manager_actions`; `manager_memory_write_flows_through_living_memory_manager` | Browser mutates state only through authenticated typed xiaod actions/managers |
| 23 | WebUI secret masking | `manager_provider_json_masks_write_only_secrets_and_header_values` | Admin responses never return raw credential/header values |
| 24 | v0.2.5 migration | `representative_v025_state_migrates_transactionally_and_idempotently`; WebUI-first owner test | Sessions/history/runs/profiles survive; stable owner rekey is transactional/idempotent and secrets stay isolated |

## Architecture coverage

### Identity, environment, and capabilities

- `OwnerIdentity` is stable across DM/group/topics; owner-global living memory,
  skills, profiles, credentials, and recall are not keyed by chat ID.
- `TelegramScope` and `XiaoSession` retain topic/session model, YOLO, active run,
  messages, and attachment isolation.
- `IdentityWorkspace` create-loads `SOUL.md`, `USER.md`, `MEMORY.md`,
  `AGENTS.md`, and `ENVIRONMENT.md` with private permissions and atomic writes.
- `RuntimeState` refreshes only generated `ENVIRONMENT.md`.
- `EnvironmentProbe` is fakeable and captures platform/Android, architecture,
  Xiao version, UID, root evidence, SELinux, Termux prefix/home/PATH/shell,
  package manager, selected binaries, and execution backends.
- `CapabilityRegistry` represents available, missing-installable,
  approval-required, temporary, unsupported, forbidden, and unknown states.
- Coverage: all `identity::tests`, `runtime::environment::tests`, and
  `runtime::capabilities::tests`, plus owner/app migration tests.

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
- Argument-aware policy requires an expiring one-shot approval bound to exact
  owner/session/run/call/tool/argument hash for destructive commands, opaque
  shell scripts, and credential-sensitive access.
- `DependencyResolver` accepts only trusted normalized package mappings,
  or validated trusted Termux repository candidates, records source/validation,
  re-probes, refreshes capability state, and then resumes.
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

### Custom profiles, attachments, and multimodal context

- Owner-global Custom profiles carry isolated endpoint, protocol, credential
  reference, safe/header references, discovered models, and verified
  capabilities. A missing credential emits no Authorization header and cannot
  inherit another profile's state.
- A selected enabled profile makes the Custom runtime available independently
  of the legacy singleton compatibility flag. Wizard commit failures restore
  the prior session selection and remove partial profile/model rows.
- Structured JSON fallback retains a bounded normalized production transcript
  across multiple tool/result turns.
- Telegram photo/document ingestion enforces pre/post size and session quota,
  MIME sniffing, image validation, SHA-256, private controlled paths, and
  session ownership.
- TXT/Markdown/source/JSON/CSV/PDF/DOCX extraction is bounded; image-only PDF
  requires OCR/vision and DOCX macros are never executed. Large document chunks
  enter attachment FTS5 and ContextEngine retrieves only relevant chunks.
- Vision serialization is provider-owned and only occurs for a model with a
  verified vision capability.
- Coverage: all `attachments::tests`, Telegram attachment ingestion,
  production Custom credential/continuation/vision tests, and context tests.

### Agent loop, verification, learning, and Telegram

- Provider capability is explicit (`Native`, `StructuredJsonFallback`, or
  `ChatOnly`). Agent-capable adapters receive canonical tools; ChatOnly action
  requests fail explicitly instead of silently becoming chat.
- `SemanticEvaluator` sends bounded redacted schema-constrained JSON requests
  with no tools, validates output, permits one bounded repair, and conservatively
  falls back without overriding deterministic policy/evidence.
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
  `/approve`, `/deny`, `/cancel`, blockers/finals, and bounded verified document
  results. `TelegramScope` retains topic IDs across sessions, menus, callbacks,
  drafts, replies, and files. Hidden reasoning has no event or persistence field.
- Coverage: `agent::completion::tests`, agent adaptive/cancel/bounds tests,
  `learning::evaluator::tests`, `owner_can_inspect_approve_and_deny_pending_operations`,
  Telegram progress/cancellation tests, and multipart document test.

### Telegram managers, doctor, and WebUI

- One registry produces the exact 17 public Telegram commands for parsing,
  `/help`, and `setMyCommands`; removed commands have no public route.
- `/model` unifies accounts, Custom profiles, and max-five model selection.
  Memory/skills managers reconcile living files/indexes before owner-facing
  lists/searches and reuse the bounded paginator.
- The Custom wizard sends a new prompt per phase, retires old keyboards, binds
  state to owner/scope/menu/expiry, never surfaces its key, scrubs recognized
  credential input from the durable Telegram inbox, and provides concrete
  phase-aware failure recovery actions.
- Doctor probes subsystems independently and labels factual evidence with
  PASS/WARN/FAIL/SKIPPED.
- Xiao Manager's eleven sections use only authenticated typed admin actions;
  secrets remain masked/write-only and logs/exports remain bounded/redacted.
- Coverage: Telegram command/login/menu/paginator tests, command manager/doctor
  tests, and IPC/WebUI manager/security tests.

### Storage, migration, recovery, and exclusions

- Migration version 10 adds `approvals`, `dependency_installs`,
  `environment_probes`, `workspace_file_index`, and `skill_file_index` after
  the v6-v9 run/memory/FTS/skill migrations. Version 11 adds Telegram scope and
  active-session state, per-session YOLO, provider capability metadata, tool
  approval audit, skill prerequisites, and dependency source validation.
  Version 12 adds learned/imported source and enabled state for skills. Version
  13 adds stable owners/migration mapping; versions 14–15 add exact approval
  identity and isolated Custom profiles/models; version 16 adds attachment
  metadata/chunks/FTS.
- `v020_migration_is_fresh_and_idempotent_with_consistent_fts` expects version
  17 and every new object/column; representative v0.2.5 and WebUI-first tests
  verify stable-owner rekey preservation/idempotency; v0.1.0 upgrade and reopen
  tests preserve history and quarantine uncertain side effects.
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
node --check module/webroot/assets/app.js
node --check module/webroot/assets/ksu-bridge.js
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
3. Configure a real Telegram bot/owner ACL; exercise DM/group/topics, new-prompt
   Custom login/error recovery, exact approval/retry, `/cancel`, YOLO,
   memory/skills/model managers, dependency progress, blocker delivery, photo,
   PDF/DOCX ingestion, vision, and document upload.
4. Complete real Codex/Antigravity OAuth and tool continuation using owner
   accounts; confirm no provider receives tools it cannot continue.
5. Exercise the typed Xiao-service inspection/restart broker under the actual
   KernelSU/Magisk service model and verify exact one-shot approval.
6. Manually edit USER.md, MEMORY.md, and a community SKILL.md, restart, and
   verify reconciliation/history without SOUL replacement.
7. Exercise every Xiao Manager section in KernelSU WebUI through the real local
   admin bridge, including task cancellation, masked provider management,
   diagnostics rerun, and safe log export.

Real credentials, Telegram delivery, Android init/root/SELinux behavior, and
device package repositories are intentionally not impersonated by host tests.

## v0.2.7 acceptance addendum

Release acceptance additionally covers canonical single-owner migration/enforcement; shared Telegram setup; explicit structured CLI with stable JSON/error/exit semantics and exact CLI sessions; CLI file/image chat; WebUI Telegram setup and exact-session AI configuration; tri-state Custom tools/structured/continuation/vision/file capabilities; scanned-PDF OCR/vision fallback after embedded extraction; attachment quota/retention/orphan/active-run protection; full Custom profile editing with credential safety; bounded live-or-CACHED Doctor probes; and Telegram/CLI/WebUI parity for provider, memory, skill, approval, diagnostics, and session AI operations. Deterministic host tests and Android arm64 cross-build/package verification are authoritative where supported; physical rooted-device/live-provider success is not inferred.
