# 15 — Test and Acceptance Matrix

## CI policy

The current PR explicitly states that Rust/WebUI/Android compile/build gates run only in GitHub Actions and local builds are prohibited. Preserve that workflow unless the repository policy changes explicitly.

Implementation agents should still add tests and inspect code locally, but must not violate repo-specific no-local-build instructions.

## A. Vision capability

1. Probe success → Supported.
2. Probe content mismatch → Unknown, not Unsupported.
3. Probe timeout/500 → Unknown.
4. Exact provider image-schema unsupported error → Unsupported.
5. Runtime image on Unknown is actually sent.
6. Runtime image success upgrades to Supported.
7. Runtime transient failure leaves Unknown.
8. ForceSupported overrides auto state.
9. ForceUnsupported prevents image request.
10. Endpoint/protocol edit invalidates automatic capability evidence.
11. Exact profile/model isolation: capability learned for model A does not leak to model B.

## B. Turn settings

1. Default `max_turns == 150`.
2. Validator accepts 150 and rejects >500.
3. WebUI GET displays 150.
4. WebUI POST changes value and new run sees new snapshot.
5. Existing active run keeps its starting settings snapshot.
6. A scripted provider can exceed 8 turns without premature failure.
7. No-progress guard still stops repeated loop around 3 repeats.

## C. Streaming

1. Chat Completions SSE text reaches frontend before completion.
2. Responses SSE text reaches frontend before completion.
3. Native streamed tool-call argument deltas assemble correctly.
4. Raw tool JSON never appears in Telegram text.
5. Reasoning fields are ignored.
6. Unknown streaming → successful stream → cache Supported.
7. Explicit unsupported streaming before data → one non-stream retry.
8. No retry after partial visible output.
9. `/stop` cancels stream.
10. Final permanent message emitted exactly once.

## D. Foreground latency

1. No provider-backed memory semantic call before first main provider request for ordinary prompts.
2. Informational answer does not call semantic completion provider.
3. Deterministically verified tool action does not call semantic completion provider.
4. Ambiguous action may call one bounded semantic verifier.
5. Final delivery acknowledgement occurs before post-delivery learning execution.
6. Background learning does not hold AgentAnswer/Telegram final.

## E. Parallel tools

1. Two read-only tools execute concurrently.
2. Results preserve original order.
3. Read-only group before mutation completes before mutation begins.
4. Reads after mutation do not move ahead of mutation.
5. Unknown/mutating tools remain sequential.
6. Cancellation interrupts all children and records state.

## F. `termux_job`

1. Multiple structured steps run under Termux UID.
2. Maximum steps enforced.
3. No `bash -c`/opaque shell string.
4. `su`/`tsu`/equivalent denied.
5. Each substep has audit evidence.
6. Aggregated result is bounded and ordered.
7. Parent cancellation kills child processes.
8. Runtime/tool timeout enforced.
9. One plan can reduce a 4-command inspection to one provider tool-call round trip.

## G. Cache

1. Same safe plan gets stable cache key.
2. Secret-bearing content is rejected from cache.
3. Environment/schema version change invalidates plan.
4. Dynamic read result tools are not result-cached by default.
5. Cached file-backed script hash is verified before execution.
6. Script cannot become root escalation path.

## H. Telegram UX

1. Existing public slash list remains stable.
2. Streaming answer updates one draft rather than sending chunk spam.
3. Progress timeline remains bounded to configured 24/30 behavior from current architecture.
4. Final message omits progress block.
5. Vision Unknown no longer causes premature local error.
6. Turn-limit error is user-useful and includes last observable state.

## I. Storage/restart

1. Migration 26→27 preserves sessions/memory/skills/profiles.
2. Pending learning job survives daemon restart.
3. Stale running learning lease recovers safely.
4. Capability override persists.
5. Timing events are linked to exact run and bounded.

## J. Exact-head release gates

Required before release/version bump:

- rustfmt;
- cargo check;
- cargo test;
- strict clippy;
- release build;
- WebUI production build;
- WebUI JS/static acceptance;
- Android arm64 build;
- deterministic package verification if workflow already requires it;
- exact final head CI success;
- real rooted Android Telegram smoke test;
- real Custom multimodal smoke test;
- real `/stop` cancellation test;
- real latency timing capture.
