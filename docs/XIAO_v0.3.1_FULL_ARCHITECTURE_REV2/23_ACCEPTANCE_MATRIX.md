# 23 — Acceptance Matrix

| Area | Requirement | Gate |
|---|---|---|
| Foreground | first provider call is main generation | P0 |
| Foreground | info prompts make 0 auxiliary semantic calls | P0 |
| Completion | deterministic evidence avoids LLM verifier | P0 |
| Sessions | `/new` inherits provider/profile/model | P0 |
| Login | alias collision auto suffixes | P0 |
| Model | one tap, no mandatory probe | P0 |
| Commands | `/provider` and `/model` separated | P0 |
| Image | Telegram photo stored privately and sent to provider | P0 |
| Image | valid photo answer never false Blocked | P0 |
| Capability | Unknown/Supported/Unsupported preserved | P0 |
| Capability | owner overrides work | P1 |
| Streaming | Chat + Responses SSE | P0 |
| Streaming | reasoning/tool args not visible | P0 |
| Telegram | one draft identity and one permanent final | P0 |
| Telegram | re-entry sees latest draft | Device |
| Telegram | GenerationCompleted no fake finishing row | P1 |
| Scheduler | mixed read-only groups parallelize | P1 |
| Termux | structured safe commands autonomous | P1 |
| Termux | root/opaque shell denied | P0 |
| Cache | production plan cache hit demonstrated | P1 |
| Cache | production script cache hit demonstrated | P1 |
| Learning | starts after delivery ACK | P0 |
| CLI | successful final output ACK releases learning | P1 |
| No-progress | changed state allows same command retry | P0 |
| PDF | parseable workspace artifact | P0 |
| PDF | long/multilingual no silent corruption | P1 |
| WebUI | current manager contracts preserved | P0 |
| WebUI | PR #5 visual features ported, not wholesale regression | P0 |
| WebUI | no remote CSS runtime dependency | P1 |
| WebUI | system/light/dark | P1 |
| WebUI | capability override controls | P1 |
| WebUI | run timing waterfall | P1 |
| Observability | first byte + first visible delta | P1 |
| Android | A–O manual acceptance | Release |
| CI | exact final SHA all jobs green | Release |
