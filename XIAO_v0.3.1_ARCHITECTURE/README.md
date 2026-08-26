# Xiao v0.3.1 — Runtime Optimization & Multimodal Hardening

This package is the architecture and implementation source-of-truth for the next Xiao hardening slice.

## Baseline

- Repository: `As-tsaqib/xiao`
- Active PR: `#2 — Xiao v0.3.0 single-binary runtime`
- Branch: `feat/v0.3.0-single-binary`
- Exact audited baseline head: `be8ccfb204e9ba512c6801f08af4ef2ef607b4e6`
- PR state at package creation: open, draft, mergeable.
- Exact-head GitHub Actions CI run: `#212` / run id `32847974963`, conclusion `success`.
- Current schema version observed at baseline: `26`.

The implementation agent MUST re-check the current PR head before editing. If the branch has advanced, treat the current branch as authoritative, diff it against the baseline above, preserve newer correct work, and adapt this architecture rather than resetting or force-reverting.

## Why v0.3.1 exists

v0.3.0 established the single-binary runtime, Custom-only active provider path, Telegram control-plane simplification, Termux workspaces, and WebUI management plane. Real-device testing exposed important runtime defects and performance problems that must be fixed before Xiao is considered fast, robust, or multimodal-ready:

1. A genuinely multimodal Custom model can be rejected before it ever receives an image because `Unknown` vision capability is projected to `vision=false` and the agent hard-gates image requests.
2. `agent.max_turns` defaults to 8 and validates only up to 32, causing ordinary agent tasks to terminate prematurely.
3. Custom provider requests currently use `stream: false`, so even fast models feel slow in Telegram.
4. Provider-backed semantic memory/verification/learning can sit on the user-visible critical path and add multiple extra LLM round trips.
5. Multiple tool calls returned in one provider turn are executed sequentially even when they are independent read-only operations.
6. Xiao lacks an efficient structured multi-step Termux job/pipeline primitive and reusable plan/script caching.
7. WebUI does not expose agent-loop limits or performance controls.
8. Runtime observability is insufficient to distinguish provider latency from Xiao orchestration latency.

## Release theme

**Fast path first, evidence preserved, security boundaries explicit.**

Xiao v0.3.1 must make normal interactions feel like the underlying model speed, while retaining Xiao's core invariants: single owner, bounded execution, observable tool evidence, no hidden chain-of-thought persistence, controlled root access, truthful capability handling, and post-success learning.

## Package map

- `00_BASELINE_AND_SCOPE.md` — exact repo baseline and non-goals.
- `01_INVARIANTS.md` — hard runtime/security/product invariants.
- `02_AGENT_LOOP_V2.md` — new high-ceiling, low-waste loop.
- `03_MULTIMODAL_CAPABILITY_ROUTING.md` — vision/file capability repair.
- `04_STREAMING_AND_FAST_RESPONSE.md` — true end-to-end streaming.
- `05_TOOL_EXECUTION_SCHEDULER.md` — parallel safe reads and ordered writes.
- `06_EXECUTION_PLAN_AND_CACHE.md` — multi-step Termux job + plan/script cache.
- `07_MEMORY_SKILLS_BACKGROUND_PIPELINE.md` — remove learning from response critical path.
- `08_CONTEXT_COMPACTION_AND_NO_PROGRESS.md` — make 150 turns safe and bounded.
- `09_AGENT_SETTINGS_WEBUI.md` — WebUI controls and hot reload.
- `10_CUSTOM_PROVIDER_RUNTIME.md` — Custom-only provider cleanup and wire behavior.
- `11_TERMUX_ROOT_POLICY.md` — Termux workshop vs root broker policy.
- `12_TELEGRAM_RUNTIME_UX.md` — draft/final behavior and command preservation.
- `13_LATENCY_OBSERVABILITY.md` — timing telemetry and diagnostics.
- `14_STORAGE_MIGRATION.md` — schema/data changes.
- `15_TEST_AND_ACCEPTANCE_MATRIX.md` — automated and real-device gates.
- `16_IMPLEMENTATION_ORDER.md` — dependency-ordered rollout.
- `17_PICOCLAW_ZEROCLAW_LESSONS.md` — borrowed patterns and boundaries.
- `18_DECISION_LOG.md` — final architectural choices.
- `XIAO_v0.3.1_MASTER_SPEC.md` — consolidated release specification.
- `XIAO_v0.3.1_AGENT_IMPLEMENTATION_PROMPT.md` — standalone implementation prompt.
- `MANIFEST.md` and `CHECKSUMS.sha256` — package integrity.
