# 16 — Observability and Latency

## Run event schema

Persist bounded events with:
- run ID;
- kind;
- elapsed_ms;
- redacted JSON detail.

Required kinds:
- `run_started`
- `context_ready`
- `pre_provider_complete`
- `provider_request_start`
- `provider_first_byte`
- `first_visible_text_delta`
- `provider_completion`
- `tool_group_start`
- `tool_group_complete`
- `verification_completed`
- `final_frontend_delivery`
- `background_learning_started`
- `background_learning_completed`
- cache hit/miss/reject events.

## Provider instrumentation

`provider_first_byte` is recorded in the adapter at first response body/SSE byte.

`first_visible_text_delta` is recorded only for user-visible text, not reasoning or tool-argument deltas.

Record once per run/turn as appropriate.

## WebUI run detail

Render:
- total foreground time;
- pre-provider overhead;
- provider TTFT;
- first visible delta;
- provider total;
- tool wall-clock;
- verification latency;
- final delivery latency;
- background learning duration;
- cache hits.

## Redaction

Do not persist:
- chain-of-thought;
- secret headers;
- API keys/tokens;
- raw oversized tool output.

Use bounded public summaries and hashes where needed.

## Operational question

The UI should make it possible to answer:

> "Why did Xiao feel slow on this request?"

without reading daemon logs.
