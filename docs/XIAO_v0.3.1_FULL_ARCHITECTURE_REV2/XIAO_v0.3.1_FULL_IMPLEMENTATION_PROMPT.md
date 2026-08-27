# Xiao v0.3.1 — Full Completion Implementation Prompt

You are implementing the final Xiao v0.3.1 release in repository `As-tsaqib/xiao`.

## Mission

Continue from the latest `main` and complete **Xiao v0.3.1 — Runtime Optimization, Multimodal Hardening, Telegram Reliability & WebUI Contract Completion**.

Do not stop at analysis or an implementation plan. Make the code changes, migrations if actually required, tests, WebUI changes, documentation, and validation updates.

## Source of truth

Read the complete `XIAO_v0.3.1_FULL_ARCHITECTURE_REV2` package before editing. In conflicts, use:

1. `XIAO_v0.3.1_MASTER_SPEC.md`
2. `01_INVARIANTS.md`
3. the relevant subsystem document
4. `23_ACCEPTANCE_MATRIX.md`

Treat old v0.3.1 docs as historical where they conflict with Revision 2.

## Current facts you must verify before editing

The previous audit of `main` identified these important conditions:

- current main had green automated CI;
- the release was not device-ready;
- task classification could still perform a semantic provider call before the main provider request;
- read-only scheduler existed but mixed provider batches were not fully scheduled by groups;
- PlanCache/CachedScript primitives existed without proven production reuse;
- capability evidence backend existed but owner override controls were absent from WebUI;
- run events existed but first byte / first visible text delta were not fully measured;
- Telegram final delivery released background learning, but CLI did not have an equivalent delivery ACK;
- Telegram `GenerationCompleted` still created a synthetic `Finishing response`;
- PR #5 contained useful visual improvements but regressed working manager contracts.

Re-audit latest main first because code may have advanced.

## User-reported bugs/behavior that are mandatory acceptance contracts

Preserve or implement:

1. `/new` must keep the current provider/profile/model binding and reset YOLO off.
2. `/login` Custom alias collisions auto-suffix `custom_1`, `custom_2`, etc.
3. `/model` selection is one-tap and must not block on an exact capability probe.
4. `/provider` and `/model` are separate.
5. Telegram Android progress uses readable Unicode fallback when custom emoji is clipped.
6. A Telegram photo such as "Apa ini" must not become false `Blocked` after valid direct vision completion.
7. Attachments are persisted in Xiao private storage and the exact stored media reaches the provider.
8. Direct-final/draft lifecycle must have one ephemeral draft identity and one permanent final.
9. Informational/code-example prompts do not trigger arbitrary Termux execution.
10. No-progress/ping-pong detection is result/state-aware.
11. `pdf_create` is deterministic, workspace-relative, parseable, and artifact-deliverable.
12. Termux path/symlink security remains hardened.
13. CLI syntax and human output remain strict.
14. Device retest must cover photo, direct-final modes, leave/re-enter streaming, PDF long/multilingual, same-command repair, and rooted Android gates.

## Execution order

### Phase 1 — protect contracts

Before risky refactors, add/repair tests that lock down the user-reported behavior and manager API contracts.

WebUI-equivalent integration tests must exercise the real daemon handlers. Do not rely only on static source-string tests.

### Phase 2 — remove hidden foreground LLM round trips

The first AI provider call for ordinary prompts MUST be the main generation call.

Replace semantic-first task classification with local deterministic classification.

Do not call a semantic task-intent model before main provider generation.

Completion is evidence-first:
- informational -> no semantic verifier by default;
- deterministic action evidence -> no semantic verifier;
- genuinely ambiguous post-main completion may use at most one bounded semantic interpretation call;
- semantic output never invents evidence.

Fix latency tests so their provider explicitly supports semantic evaluation. A mock whose default `supports_semantic_evaluation=false` is not a valid proof that production avoids semantic calls.

### Phase 3 — scheduler

Wire `ToolExecutionScheduler` into AgentEngine for mixed batches.

Required behavior:

```text
read A + read B -> parallel
write C         -> barrier
read D + read E -> parallel
```

Preserve provider call order in returned ToolResults.

Propagate cancellation and durable interrupted status.

### Phase 4 — production cache use

Do not count cache structs as complete.

Wire safe plan reuse into actual `termux_job`/execution path:
- schema version;
- environment fingerprint;
- secret rejection;
- policy revalidation;
- hit/miss telemetry.

Implement real file-backed script cache reuse where appropriate:
- trusted interpreter;
- SHA256;
- provenance;
- no secrets;
- no root escalation;
- policy re-check.

Add tests that demonstrate a real second execution takes a cache-hit path.

### Phase 5 — Termux policy

Keep hard DENY:
- `su`, `tsu`, `sudo`, `doas`;
- shell `-c`;
- opaque shell strings;
- secret exfiltration;
- workspace escape.

Allow routine structured unprivileged commands.

For destructive filesystem commands:
- auto-allow only when all targets canonicalize inside Xiao session workspace;
- outside workspace -> ASK;
- sensitive/system/root -> DENY or typed AndroidBroker.

Verified Xiao cached scripts may run without repetitive approval; arbitrary scripts require exact approval.

### Phase 6 — multimodal and model selection

Model activation must not wait for Probe.

Capability states:
`Supported | Unsupported | Unknown`.

Owner overrides:
`auto | force_supported | force_unsupported`.

Runtime success upgrades Unknown to Supported.
Explicit exact unsupported may set Unsupported.
Transient failures remain Unknown.

Ensure the Telegram photo path sends the actual stored image and direct visual inspection can verify without a side-effect tool.

### Phase 7 — Telegram streaming/timeline

Keep one stable draft ID.

`direct_final=true` may show visible TextDelta in draft.
`direct_final=false` shows progress only.

Both:
- clear draft once;
- send exactly one permanent final.

Do not stream hidden reasoning.

Fix `GenerationCompleted`:
- finalize current Writing row;
- no synthetic active `Finishing response`.

Retain 24 normal / 30 detailed rows and ~3500 char budget.

Use Unicode fallback on Android.

Preserve heartbeat so leaving/reopening chat receives current draft state on the next update.

### Phase 8 — delivery ACK

Create a shared idempotent frontend delivery acknowledgement service.

Telegram calls it after final delivery.
CLI calls it only after final stdout/file delivery succeeds.

Background learning jobs become claimable only after ACK.

### Phase 9 — WebUI and PR #5

Audit PR #5 head before using it.

**Do not merge/replace App.jsx from PR #5 wholesale.**

Port the visual/UX improvements onto the latest functional main WebUI.

Keep:
- Android safe-area;
- dark theme;
- touch feedback;
- optimistic toggles with rollback;
- back navigation;
- refresh animation;
- full-width mobile layout where desired.

Fix these PR #5 regressions:

- never use session action `change_ai`; use canonical `ai_config`;
- keep the real `SessionAiDialog`;
- attachment removal uses `remove`;
- memory "Forget" uses daemon's exact delete payload with scope/category/key;
- Custom profile edit uses `edit`;
- safe headers are sent as `headers` object;
- secret headers are sent as object, never raw JSON string;
- show only backend-supported Custom protocols (`openai_chat_completions`, `openai_responses`) unless you implement and test another protocol end-to-end;
- preserve endpoint-change keep/clear trust-boundary controls;
- manual refresh uses a typed page->resource reload map;
- implement true theme preference `system|light|dark`;
- remove remote `https://mui.kernelsu.org/internal/insets.css` import; use local `/internal/insets.css`;
- do not force 38px top padding on non-embedded contexts;
- restore `prefers-reduced-motion`;
- icon-only bottom tabs require accessible names.

Add model capability UI:
- agent readiness;
- vision/file/streaming state;
- evidence;
- Auto / Force Supported / Force Unsupported controls for vision and file input;
- separate optional Probe button.

Model selection in WebUI must target an explicit session. Never silently mutate the first session.

Add run timing UI using daemon `timings` data, including first byte and first visible delta once implemented.

Treat `webui/src` as source of truth. Rebuild `module/webroot` from source; do not hand-maintain minified generated assets.

### Phase 10 — PDF

Keep containment and deterministic parsing.

Long PDFs must paginate.

Do not silently corrupt unsupported multilingual text. Either:
- use a verified Unicode-capable font path; or
- return an explicit unsupported-glyph warning/error.

Test artifact delivery.

### Phase 11 — validation

Run exact-head GitHub Actions:
- rustfmt
- cargo check
- cargo test
- strict clippy
- release build
- WebUI build
- JS syntax
- static acceptance
- Android arm64
- deterministic module ZIP

Then perform/record real rooted Android gates from `20_REAL_DEVICE_ACCEPTANCE.md`.

Do not claim those gates PASS without real device evidence.

## Version rule

Do NOT bump version to 0.3.1 until:
- all P0/P1 acceptance items are satisfied;
- exact-head CI is green;
- required rooted Android gates pass.

Only then update:
- Cargo/package version;
- README/About;
- `docs/V031_VALIDATION.md` with exact final SHA, exact CI run, and device evidence.

## Final report

When finished, report:
- exact final SHA;
- files changed;
- migrations;
- tests added;
- CI run;
- remaining device/manual gates if any;
- any explicit waiver.

Do not report "complete" while mandatory gates remain unverified.
