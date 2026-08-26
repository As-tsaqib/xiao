# 18 — Decision Log

## D1 — Default max turns becomes 150

Accepted. The old default 8 is too small. Hard valid maximum becomes 500. No-progress/runtime/tool/context budgets remain mandatory.

## D2 — Unknown multimodal capability is not Unsupported

Accepted. Unknown image/file capability gets an optimistic real request unless the owner forces Unsupported.

## D3 — Vision probe is positive-evidence oriented

Accepted. Failure to solve an OCR challenge is not evidence that the endpoint cannot accept images.

## D4 — Custom streaming defaults ON/Auto

Accepted. Use SSE when supported, with safe non-stream fallback only before partial output/tool protocol has been consumed.

## D5 — Final user response must not wait for skill learning

Accepted. Skill/memory learning becomes post-delivery background work with durable idempotent jobs.

## D6 — Deterministic-first completion

Accepted. Semantic provider verification is an escalation path, not the default for every turn.

## D7 — Independent read-only tools may run in parallel

Accepted. Mutating and unknown tools remain sequential in v0.3.1.

## D8 — Add `termux_job`

Accepted. It is a structured multi-step CLI tool designed to reduce provider round trips.

## D9 — Cache plans/scripts, not dynamic observations by default

Accepted. Dynamic RAM/process/network data must stay fresh. Plan cache and trusted file-backed script cache are separate from result cache.

## D10 — Termux is the autonomous workshop; root is the approval boundary

Accepted with guardrails. Structured Termux commands under the Termux UID do not require routine approval. Direct privilege escalation remains hard-denied. Root mutations use typed AndroidBroker approval/YOLO rules.

## D11 — No new Telegram settings commands

Accepted. Agent tuning lives in WebUI/CLI control plane. Telegram stays daily-use focused.

## D12 — Keep single binary

Accepted. v0.3.1 continues `xiao` + `xiao daemon`; no return to separate `xiaod` executable.
