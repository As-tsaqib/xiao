# 16 — Dependency-Ordered Implementation

Do not implement these as random independent patches. Use this order.

## Phase 0 — Re-audit current head

- fetch current PR #2 exact head;
- compare to baseline `be8ccfb...`;
- identify any newer work that already solves part of v0.3.1;
- update tests/plan without reverting correct newer code.

## Phase 1 — Settings foundation

1. Extend AgentConfig and validation ranges.
2. Default max turns 150.
3. Add runtime settings snapshot/hot-reload service.
4. Add WebUI Agent section and manager API.
5. Tests for settings and snapshot behavior.

This phase makes later features configurable without multiple config refactors.

## Phase 2 — Multimodal correctness (release blocker)

1. Separate Unknown from Unsupported in effective runtime gate.
2. Add capability overrides/evidence.
3. Normalize provider capability errors.
4. Optimistic image attempt for Unknown.
5. Runtime success learning.
6. Probe redesign.
7. WebUI exact-model override/status.
8. Tests.

## Phase 3 — Streaming fast path (release blocker)

1. Provider streaming event abstraction.
2. Chat Completions SSE.
3. Responses SSE.
4. tool-call delta assembly.
5. Telegram draft accumulated text.
6. streaming capability/fallback.
7. cancellation tests.

## Phase 4 — Remove semantic work from critical path

1. deterministic-first request/task flow;
2. local fast memory intent + queue;
3. deterministic completion fast path;
4. background learning jobs;
5. foreground/background semantic priority;
6. delivery ordering tests.

## Phase 5 — Tool scheduler

1. execution class metadata;
2. group consecutive read-only calls;
3. bounded parallel execution;
4. stable ordered results;
5. cancellation/audit tests.

## Phase 6 — `termux_job`

1. schema + bounds;
2. substep audit;
3. structured argv execution;
4. root escalation deny;
5. aggregated results;
6. cancellation;
7. tests.

## Phase 7 — Plan/script cache

1. structured plan cache;
2. invalidation fingerprint;
3. trusted/file-backed script cache;
4. secret scanning/redaction;
5. optional cache metrics.

Do not add output caching broadly in this phase.

## Phase 8 — Context/loop compaction

1. run checkpoint;
2. context pressure triggers;
3. provider continuation reset/recovery where safe;
4. enhanced no-progress/ping-pong detection;
5. long scripted loop tests.

## Phase 9 — Latency observability

Can begin earlier, but complete here:

- run event timing storage;
- WebUI run timing view;
- CLI JSON;
- end-to-end ordering tests.

## Phase 10 — Custom-only cleanup

- isolate legacy provider migration types;
- remove unused active validation/runtime paths;
- keep archival history readable.

Do this after the critical provider path is stable to avoid mixing cleanup with streaming/vision bugs.

## Phase 11 — Release qualification

- exact-head CI;
- Android real-device validation;
- Custom multimodal test;
- long multi-tool task;
- streaming latency measurement;
- version/docs bump to 0.3.1 only when all gates pass.
