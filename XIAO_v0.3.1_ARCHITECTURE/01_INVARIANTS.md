# 01 — Hard Invariants

## Product identity

Xiao remains a private, single-owner personal AI agent for rooted Android, Telegram-first, with CLI and WebUI as shared control surfaces.

## Capability truth

- `Unknown` is never silently equivalent to `Unsupported`.
- A capability may become `Supported` from a successful real request.
- A capability may become `Unsupported` only from explicit, normalized provider evidence or explicit owner override.
- Transient provider errors, timeouts, rate limits, malformed model output, or OCR task failure MUST NOT permanently downgrade vision/file capability.
- Capability state is scoped to exact Custom profile + exact model + wire protocol.

## Agent loop

- `max_turns=150` is a hard emergency ceiling, not a target.
- Xiao should stop much earlier on verified success, real blocker, cancellation, runtime budget, repeated no-progress, or context budget exhaustion.
- Increasing turn budget must not disable no-progress detection.
- Every provider/tool loop remains cancellable.

## Streaming

- User-visible assistant text may stream to a Telegram draft.
- Hidden provider reasoning / chain-of-thought is never forwarded, stored, or rendered.
- Tool-call JSON deltas are internal protocol state, never user-visible raw text.
- Final permanent Telegram output is sent exactly once after final answer completion.

## Tool execution

- Every tool call and every substep in a batch/pipeline passes ToolRegistry + ToolPolicy.
- A pipeline is an orchestration primitive, not a security bypass.
- Read-only calls may run concurrently only when the runtime can prove they are safe to parallelize.
- Mutating calls remain sequential in v0.3.1.
- Results returned to the provider preserve original call order.

## Termux vs root

- General Termux commands execute as the Termux UID, never as root.
- Direct privilege escalation from Termux (`su`, `tsu`, equivalent) is forbidden.
- Root operations are exposed only through typed AndroidBroker tools.
- Normal structured Termux commands do not require interactive approval merely because they have side effects inside the Termux workshop.
- Root/privileged broker actions use smart approval when YOLO is off; YOLO may auto-grant ASK for the current session, but cannot bypass hard DENY.

## Learning

- USER/MEMORY/skills learning remains based only on durable facts and verified observable traces.
- Learning failure must never turn a successfully delivered answer into a failed task.
- Background learning is idempotent and durable enough to survive daemon restart.

## Secrets

- No API key, bot token, secret header, sensitive path content, or hidden model reasoning may enter progress text, timeline summaries, caches, plan hashes, learning traces, or telemetry.
