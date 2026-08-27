# 19 — Test Strategy

## Unit

### Agent latency
- no pre-main semantic call;
- informational semantic calls = 0;
- deterministic action verifier calls = 0;
- max one semantic completion fallback.

### Scheduler
- mixed read/read/write/read groups;
- stable result order;
- cancellation;
- approval barrier.

### Cache
- actual runtime PlanCache hit/miss;
- invalidation by environment;
- secret rejection;
- script hash mismatch;
- root escalation rejection.

### Multimodal
- Unknown → successful exact image → Supported;
- explicit unsupported → Unsupported;
- transient error stays Unknown;
- ForceSupported attempts;
- ForceUnsupported blocks;
- profile isolation.

### Timeline
- no synthetic Thinking after tool completion;
- GenerationCompleted finalizes current Writing;
- 24/30 row retention;
- exact call-ID completion;
- Unicode fallback.

### No-progress
- A-B-A-B identical loop blocked;
- same command with changed output/state allowed.

## Integration

### WebUI contract tests
Static strings are insufficient. Exercise manager handlers with WebUI-equivalent payloads:
- `ai_config`;
- `remove` attachment;
- memory `delete` scope/category/key;
- profile `edit` with map headers;
- capability override;
- probe;
- agent settings.

Fail CI if App source contains deprecated wire actions such as `change_ai` or profile `update`.

### Protocol list
Fail CI if WebUI advertises a Custom protocol not accepted by backend validation.

### Streaming
- Chat SSE delta;
- Responses SSE delta;
- tool-call delta assembly;
- no reasoning leakage;
- fallback before partial only;
- draft clear before one permanent final.

### Attachment
- Telegram photo persists privately;
- provider receives exact image;
- answer not false Blocked.

### PDF
- parseable;
- multi-page;
- workspace containment;
- unsupported glyph behavior explicit.

### Frontend delivery
- Telegram release learning after final send;
- CLI release after successful output ACK;
- failed delivery leaves learning pending.

## CI

Exact-head:
- rustfmt;
- cargo check;
- cargo test;
- strict clippy;
- release build;
- WebUI source build;
- JS syntax;
- static contract checks;
- Android arm64;
- deterministic module ZIP verification.
