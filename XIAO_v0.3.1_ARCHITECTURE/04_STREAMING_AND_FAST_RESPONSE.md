# 04 — Streaming and Fast Response Path

## Baseline defect

Custom Chat Completions and Responses payloads currently force:

```json
"stream": false
```

This means Xiao waits for the entire upstream response before Telegram can show user-facing answer text. A fast model can therefore feel slow.

## Provider streaming contract

Introduce an internal streaming event abstraction independent of Telegram:

```rust
enum ProviderStreamEvent {
    Status(SafeStatus),
    TextDelta(String),
    ToolCallDelta(ToolCallDelta),
    Usage(UsageSnapshot),
    Completed(ProviderTurn),
}
```

Rules:

- `TextDelta` is only user-visible answer text.
- Reasoning/thinking fields from upstream protocols are discarded unless they are transformed into an allowed high-level status by Xiao itself.
- `ToolCallDelta` is accumulated internally and never rendered raw.
- The completed event still returns canonical `ProviderStep::{Final,ToolCalls}` for compatibility with AgentEngine.

## OpenAI-compatible Chat Completions

When streaming is enabled:

- send `stream: true`;
- parse SSE incrementally;
- accumulate assistant text deltas;
- accumulate native function-call name/argument deltas by stable index/call id;
- terminate on the protocol's terminal event, not merely TCP close;
- validate final tool-call JSON before ToolRegistry sees it.

## OpenAI-compatible Responses

Implement equivalent SSE handling for Responses-compatible Custom endpoints:

- accumulate output text deltas;
- accumulate function-call arguments;
- recognize terminal response events;
- normalize into the same canonical ProviderTurn.

Do not expose provider-specific event names above the adapter.

## Graceful fallback

Streaming support is also a capability state:

```text
Supported | Unsupported | Unknown
```

Default `provider_streaming=true` means **Auto try streaming**.

- `Supported`: stream directly.
- `Unknown`: attempt streaming.
- Explicit protocol rejection before any usable content: retry once non-streaming and cache `Unsupported` for exact profile/model/protocol.
- Transient errors: do not permanently downgrade.
- If any visible text or tool-call protocol data has already been consumed, do not automatically duplicate the request via fallback.

## Telegram rendering

Existing draft transport remains the transport authority.

Draft may contain:

```text
✓ inspected runtime
✓ command exited 0
✦ Writing response…

<accumulated user-visible answer text>
```

The permanent message is still sent once at completion using the normal rich renderer. Draft progress remains ephemeral.

Do not create one timeline row per text delta. Update one active Writing row and one accumulated draft answer block.

## Fast response critical path

Desired foreground path:

```text
message received
  ↓
attachment normalize / local deterministic preflight
  ↓
context build
  ↓
FIRST provider request
  ↓ SSE
first visible token ───────────→ Telegram draft
  ↓
(optional tools + continuation streams)
  ↓
fast hard-evidence verification
  ↓
permanent final response
  ↓
post-delivery learning queue
```

## Semantic work priority

Provider-backed semantic work must not compete equally with foreground generation.

Define work classes:

```text
ForegroundGeneration   highest
ForegroundVerification medium (only when required)
PostDeliveryLearning   low
```

Background learning waits while the same profile/model has active foreground generation, or is globally limited to low concurrency.

## Performance acceptance

Automated tests use mock delayed SSE endpoints and assert ordering, not fragile real-world millisecond thresholds:

- first `TextDelta` reaches frontend before upstream completion;
- final send occurs after completed stream;
- background learning does not run before final delivery acknowledgement;
- streaming fallback makes at most one safe retry;
- cancellation stops stream promptly.
