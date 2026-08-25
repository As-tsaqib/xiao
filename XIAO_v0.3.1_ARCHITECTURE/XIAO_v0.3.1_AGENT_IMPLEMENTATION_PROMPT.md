# XIAO v0.3.1 — IMPLEMENTATION PROMPT

You are implementing Xiao v0.3.1 in repository `As-tsaqib/xiao`.

## Mission

Continue from the repository's latest current state and implement **Xiao v0.3.1 — Runtime Optimization & Multimodal Hardening**. Do not stop at analysis or a plan. Make the code changes, tests, migrations, WebUI changes, and documentation required by the architecture package.

## Source of truth

Read the complete v0.3.1 architecture package before editing, especially:

1. `XIAO_v0.3.1_MASTER_SPEC.md`
2. `01_INVARIANTS.md`
3. `02_AGENT_LOOP_V2.md`
4. `03_MULTIMODAL_CAPABILITY_ROUTING.md`
5. `04_STREAMING_AND_FAST_RESPONSE.md`
6. `05_TOOL_EXECUTION_SCHEDULER.md`
7. `06_EXECUTION_PLAN_AND_CACHE.md`
8. `07_MEMORY_SKILLS_BACKGROUND_PIPELINE.md`
9. `09_AGENT_SETTINGS_WEBUI.md`
10. `15_TEST_AND_ACCEPTANCE_MATRIX.md`
11. `16_IMPLEMENTATION_ORDER.md`

If existing repository code conflicts with this package, this v0.3.1 package is the target architecture unless newer current-head code already implements a stricter/correct superset.

## Baseline awareness

This package was authored against PR #2 branch `feat/v0.3.0-single-binary` exact head:

`be8ccfb204e9ba512c6801f08af4ef2ef607b4e6`

At that head PR #2 was open/draft/mergeable and exact-head CI run #212 succeeded.

FIRST:

- inspect the current PR #2 head;
- if it has moved, review commits/diff since the baseline;
- preserve all newer correct work;
- never force-reset the branch to the baseline hash.

## Repo-specific validation policy

The PR currently states that Rust/WebUI/Android compile/build gates run only in GitHub Actions and local builds are prohibited. Respect that repository policy. Do not run prohibited local `cargo`/WebUI/Android build commands. Add tests and push/validate through the configured CI workflow instead. If the repository policy has explicitly changed at current head, follow the newer checked-in policy.

## Required implementation outcomes

### P0 — Multimodal capability correctness

Fix the real-device bug where a multimodal Custom model is rejected before receiving an image.

- Preserve tri-state capability semantics.
- Unknown != Unsupported.
- Unknown vision/file may make an optimistic real request.
- Successful real request persists Supported for exact profile/model/protocol.
- Only normalized explicit provider unsupported evidence may persist Unsupported automatically.
- Timeouts, 429, 5xx, malformed answer, or failed OCR challenge stay Unknown.
- Add owner override Auto / ForceSupported / ForceUnsupported in WebUI.
- Redesign vision probe so a content/OCR mismatch does not become negative capability evidence.
- Endpoint/protocol changes invalidate automatic capability evidence.
- Add all regression tests from the architecture.

### P0 — Agent turn ceiling + settings

- Change default `agent.max_turns` from 8 to 150.
- Change validation range to allow 150, target `2..=500`.
- Preserve explicit owner config values during migration when possible.
- Add WebUI Agent section with max turns, max tool calls, runtime timeout, no-progress threshold, provider streaming, parallel reads, execution-plan controls, and background learning.
- Settings are saved atomically and hot-reloaded for NEW runs using a run-start snapshot; active run settings do not mutate underneath the run.

### P0 — True Custom provider streaming

Implement end-to-end streaming for both configured OpenAI-compatible protocol families.

- Add canonical internal stream events.
- Chat Completions SSE text/tool-call delta assembly.
- Responses-compatible SSE text/tool-call delta assembly.
- Never surface provider hidden reasoning or raw tool JSON.
- Telegram draft shows accumulated user-visible answer text while generation continues.
- Final permanent reply is sent once.
- Streaming Unknown tries stream; explicit unsupported before partial output may retry once non-stream.
- No duplicate retry after partial output/tool-call data.
- `/stop` cancels stream promptly.

### P0 — Remove semantic LLM work from normal response critical path

- Do not block first main provider request on provider-backed memory semantic evaluation.
- Introduce local deterministic fast memory intent/pending overlay as needed.
- Queue durable semantic memory reconciliation.
- Completion verification becomes deterministic-first and only escalates ambiguous cases.
- Skill/memory learning after verified success becomes post-delivery background work.
- Background jobs are durable, idempotent, restart-safe, bounded, and low-priority.
- Learning failure cannot retroactively fail a delivered task.

### P1 — Smart tool execution scheduler

- Extract tool execution scheduler.
- Consecutive statically read-only calls may run concurrently with bounded semaphore.
- Mutation/unknown/approval calls remain sequential.
- Preserve result order/call IDs.
- Preserve independent audit rows/progress/cancellation.

### P1 — `termux_job`

Implement structured multi-step Termux workflow tool.

- bounded step count default 32/hard <=64;
- structured program + argv only;
- no arbitrary shell command strings;
- no `bash -c` / `sh -c` model path;
- no `su`/`tsu`/root escalation;
- every substep policy/executor/audit path remains canonical;
- aggregated bounded ToolResult;
- cancellation kills children;
- provider can accomplish common multi-command inspection in one tool call.

### P1 — Plan/script caching

- Add structured plan cache with stable content/environment/schema hash.
- Never include secrets.
- Add file-backed script cache only for trusted/template/verified workflow scripts with content hash and auditable interpreter/path.
- Do NOT broadly cache dynamic command results.
- Live RAM/process/network/git observations default Never-cache.

### P1 — 150-turn safety

- Add/strengthen result-aware no-progress and ping-pong detection.
- Add bounded observable run checkpoint/context compaction.
- Raw audit remains in DB; provider context is compacted.
- High ceiling must not create 150-turn runaway loops.

### P1 — Latency observability

Persist and expose timing events for:

- pre-provider overhead;
- provider request start;
- first byte;
- first visible text delta;
- provider completion;
- tool groups;
- verification;
- final answer ready;
- final frontend delivery;
- background learning.

Show useful timing on WebUI Runs and stable CLI JSON when possible. Never put prompts/secrets/reasoning into timing metadata.

### P2 — Custom-only cleanup

After critical behavior is stable:

- keep active ProviderRegistry Custom-only;
- isolate legacy Codex/Antigravity migration readers/types from normal runtime paths;
- do not let unused legacy provider config validation break a Custom-only installation;
- preserve archived history and explicit migration behavior;
- do not re-add provider Telegram commands.

## Termux/root policy target

Use Termux as Xiao's autonomous non-root workshop:

- ordinary structured Termux commands: ALLOW without interactive approval;
- direct privilege escalation from Termux: hard DENY;
- root operations: typed AndroidBroker only;
- privileged mutations: ASK when YOLO off, auto-grant ASK in current YOLO session with audit, hard DENY always remains DENY;
- cached scripts/plans cannot bypass this boundary.

Do NOT create `/approve` or `/deny` Telegram slash commands.

## Telegram preservation

Do not expand Telegram settings surface. Preserve the simplified v0.3 daily-use command registry. Do not reintroduce `/memory`, `/doctor`, `/approvals`, `/provider`, `/account`, or legacy `/session` into public help/menu.

## Required tests

Implement the full matrix in `15_TEST_AND_ACCEPTANCE_MATRIX.md`, including at minimum:

- Unknown vision real request succeeds and self-upgrades capability;
- explicit unsupported image response downgrades only exact model/profile/protocol;
- default turns 150 and WebUI setting applies to new runs;
- old 8-turn failure regression test;
- SSE text arrives before complete response;
- streamed tool-call assembly;
- no reasoning leak;
- final response before background learning;
- read-only tool concurrency with stable result order;
- mutation barriers;
- `termux_job` security/cancellation/audit;
- plan cache secret/invalidation tests;
- no-progress early stop despite max_turns 150;
- migration/restart of learning jobs;
- latency event ordering.

Prefer deterministic fake/mock provider tests. Do not make CI depend on a real Gemini/OpenAI endpoint.

## Implementation discipline

- Work in dependency order from `16_IMPLEMENTATION_ORDER.md`.
- Add RED regression tests before each bug fix where repo policy expects that pattern.
- Do not weaken ToolPolicy, secret handling, cancellation, or evidence verification just to make tests pass.
- Do not persist chain-of-thought.
- Do not fabricate provider capabilities from model names.
- Keep exact profile/model/protocol isolation.
- Keep one shipped binary `xiao`; daemon remains `xiao daemon`.
- Do not add MCP/subagents/vector DB/cron/browser platform/provider families.

## Completion requirements

Do not declare v0.3.1 complete until:

1. all architecture acceptance tests are implemented;
2. exact final head CI passes all configured Rust/WebUI/Android gates;
3. validation docs reflect the exact final SHA/run, not stale examples;
4. real rooted Android acceptance is documented for Telegram streaming, vision, `/stop`, and Termux job;
5. version/docs are updated to 0.3.1 only after implementation gates pass.

At the end, report:

- final exact commit SHA;
- CI run id/conclusion;
- files/modules changed;
- migration version;
- tests added;
- real-device items completed vs still manual;
- any remaining blocker with concrete evidence.
