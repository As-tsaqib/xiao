# Xiao v0.3.1 Full Architecture Revision 2

**Release theme:** Runtime Optimization, Multimodal Hardening, Telegram Reliability, and WebUI Contract Completion.

This package is the revised source of truth for completing Xiao v0.3.1 from the current `main` state. It incorporates:

- the v0.3.1 architecture decisions already agreed;
- the latest audit of `main` at `93f9f54255783c4e28fadbb1110f6620e30ade40`;
- the user-reported Telegram/device bugs and fixes that still require live retest;
- review of PR #5 (`feat/rewrite-xiao-webui`, head `85fee2b925172c0313632c4f9b557137bd646097`);
- a stricter WebUI contract so visual rewrites cannot regress daemon behavior;
- explicit release gates for real rooted Android.

## Authority

When documents conflict, use this order:

1. `XIAO_v0.3.1_MASTER_SPEC.md`
2. `01_INVARIANTS.md`
3. subsystem specification matching the code being changed
4. `23_ACCEPTANCE_MATRIX.md`
5. historical v0.3.x / v0.2.x docs

No implementation is considered complete merely because unit tests pass. Release readiness requires the real-device gates in `20_REAL_DEVICE_ACCEPTANCE.md`.

## Important current-state conclusion

The current `main` is substantially implemented and automated CI is green, but v0.3.1 is **not release-ready** until the remaining architecture gaps are closed. In particular:

- no auxiliary semantic LLM request may delay the first main provider request for ordinary prompts;
- mixed tool batches must use the real read-only scheduler, not all-or-nothing parallelization;
- plan/script caches must be used by production runtime, not exist only as primitives;
- WebUI must expose capability overrides and preserve exact IPC contracts;
- first-byte / first-visible-delta latency must be measured;
- background learning delivery acknowledgement must work for all frontends;
- Telegram timeline completion must not add synthetic "Finishing response";
- PR #5 must not be merged as-is because its source rewrite regresses multiple working control-plane contracts.

## Package map

- `01_INVARIANTS.md` — hard product/security/runtime invariants
- `02_AGENT_LOOP_V2.md` — foreground agent loop and completion semantics
- `03_FOREGROUND_LATENCY.md` — deterministic-first fast path and timing budget
- `04_MULTIMODAL_CAPABILITY_ROUTING.md` — tri-state vision/file/streaming truth
- `05_STREAMING_AND_TELEGRAM_DRAFTS.md` — SSE and Telegram draft/final lifecycle
- `06_TOOL_EXECUTION_SCHEDULER.md` — safe concurrency and barriers
- `07_EXECUTION_PLAN_AND_CACHE.md` — `termux_job`, plan cache, script cache
- `08_TERMUX_ANDROID_SECURITY.md` — Termux vs typed Android broker policy
- `09_MEMORY_SKILLS_BACKGROUND_LEARNING.md` — post-delivery learning pipeline
- `10_SESSIONS_PROVIDER_MODEL_COMMANDS.md` — `/new`, `/login`, `/provider`, `/model`
- `11_ATTACHMENTS_IMAGES_PDF.md` — private attachments, images, scanned PDF, PDF create
- `12_TELEGRAM_UX_BUGFIXES.md` — reported Telegram bugs and behavior
- `13_WEBUI_INFORMATION_ARCHITECTURE.md` — desired Xiao Manager layout
- `14_WEBUI_PR5_REVIEW.md` — concrete PR #5 audit
- `15_WEBUI_API_CONTRACTS.md` — exact daemon/UI contracts
- `16_OBSERVABILITY.md` — latency events, run traces, cache telemetry
- `17_CLI_AND_FRONTEND_DELIVERY.md` — CLI parity and delivery ACK
- `18_STORAGE_AND_MIGRATION.md` — durable schema rules
- `19_TEST_STRATEGY.md` — host/integration test matrix
- `20_REAL_DEVICE_ACCEPTANCE.md` — rooted Android acceptance suite
- `21_RELEASE_GATES.md` — version promotion rules
- `22_IMPLEMENTATION_ORDER.md` — dependency-ordered execution plan
- `23_ACCEPTANCE_MATRIX.md` — checklist
- `XIAO_v0.3.1_FULL_IMPLEMENTATION_PROMPT.md` — implementation-agent prompt
