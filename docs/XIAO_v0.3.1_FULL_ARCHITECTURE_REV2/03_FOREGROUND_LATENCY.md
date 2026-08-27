# 03 — Foreground Latency

## Primary objective

The gap between "model is fast" and "Xiao feels slow" must be measurable and minimized.

## Hard fast-path rule

For an ordinary prompt, the first network call to an AI provider must be the main user generation request.

Forbidden pre-main-provider work:
- semantic task-intent LLM call;
- semantic memory write evaluation;
- skill synthesis;
- completion verification;
- capability probe unless the owner explicitly requested Probe;
- model discovery.

Allowed pre-main work:
- SQLite/session reads;
- local deterministic task classification;
- bounded context retrieval;
- attachment lookup;
- local capability lookup;
- local policy evaluation.

## Target timing events

Every run records monotonic elapsed milliseconds:

1. `run_started`
2. `context_ready`
3. `pre_provider_complete`
4. `provider_request_start`
5. `provider_first_byte`
6. `first_visible_text_delta`
7. `provider_completion`
8. `tools_started` / `tools_completed` as needed
9. `verification_completed`
10. `final_frontend_delivery`
11. `background_learning_started`
12. `background_learning_completed`

## Interpretation

Example:

```text
pre_provider_complete        32 ms
provider_request_start       34 ms
provider_first_byte         180 ms
first_visible_text_delta    240 ms
provider_completion        1280 ms
verification_completed     1295 ms
final_frontend_delivery    1370 ms
background_learning        3100 ms
```

This lets the operator distinguish Xiao overhead from provider TTFT.

## Test requirements

- A provider fixture with semantic evaluation enabled must still observe `main_provider_turn_0` as the first provider call for an informational prompt.
- Informational prompt: auxiliary semantic call count = 0.
- Deterministically verified action: auxiliary semantic call count = 0.
- Ambiguous completion may use at most one post-main semantic call.
- `provider_first_byte <= first_visible_text_delta <= provider_completion`.
