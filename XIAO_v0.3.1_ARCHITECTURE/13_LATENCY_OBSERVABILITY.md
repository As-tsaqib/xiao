# 13 — Latency and Runtime Observability

## Goal

When a user says “the model is Flash but Xiao is slow,” Xiao must be able to show whether time was spent in provider generation, local orchestration, tools, verification, or learning.

## Run timing events

Record monotonic durations / timestamps for at least:

```text
message_received
run_started
context_build_started/completed
provider_request_started
provider_first_byte
provider_first_text_delta
provider_turn_completed
tool_group_started/completed
verification_started/completed
final_answer_ready
frontend_final_send_started/completed
learning_enqueued
learning_started/completed
```

## Derived metrics

- pre-provider overhead;
- upstream TTFT;
- time to first Telegram draft answer text;
- provider turn duration;
- cumulative provider time;
- tool wall time vs sum of tool execution times;
- verification latency;
- final delivery latency;
- post-delivery learning latency;
- total user-visible latency.

## Storage

Prefer compact timing rows/events linked to `agent_run_id`. Do not store prompt bodies or secret payloads in telemetry.

## WebUI Runs

Run detail should show a timing waterfall or compact table, for example:

```text
Preflight                 42 ms
Provider TTFT            310 ms
Provider total          1250 ms
Tools                    480 ms (3 calls, 2 parallel)
Verification              18 ms
Final delivery            70 ms
Background learning     2400 ms (not user blocking)
```

## CLI

`xiao runs show <id> --json` should expose stable timing fields when available.

## Logs

Add redacted event log messages with run id and elapsed duration. Never log streamed answer text merely for timing.

## Acceptance

Tests assert sequencing with a mock clock/delayed fake provider. Real-device acceptance captures one no-tool request and one tool request and confirms background learning is not included in user-visible final latency.
