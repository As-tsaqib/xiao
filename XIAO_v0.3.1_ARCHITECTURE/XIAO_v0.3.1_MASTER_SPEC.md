# XIAO v0.3.1 MASTER SPEC

## 1. Release title

**Xiao v0.3.1 — Runtime Optimization & Multimodal Hardening**

## 2. Baseline

Implement from the latest state of `As-tsaqib/xiao` PR #2 (`feat/v0.3.0-single-binary`). Architecture was authored against exact head:

`be8ccfb204e9ba512c6801f08af4ef2ef607b4e6`

At that head CI run #212 succeeded. If the branch moved, inspect current head first and adapt; never reset newer correct work to the baseline hash.

## 3. Release objective

v0.3.1 turns the v0.3 single-binary foundation into a responsive agent runtime suitable for real Telegram use with Custom OpenAI-compatible endpoints. It must fix false multimodal rejection, raise the loop ceiling to 150 without permitting wasteful loops, stream user-visible output, cut redundant provider calls, execute independent tools efficiently, and move memory/skill learning out of the user-facing latency path.

## 4. User-observed defects that are release blockers

### 4.1 Multimodal model rejected as non-vision

Current behavior can map a failed/inconclusive vision probe to an effective boolean false. The agent then rejects an image locally before the provider receives it. This is incorrect for a Custom model whose capability is simply unknown.

Required behavior:

- tri-state capability remains authoritative;
- Unknown is optimistic-try;
- successful real image request upgrades exact model/profile/protocol to Supported;
- only explicit provider unsupported evidence downgrades to Unsupported;
- owner can override Auto/Supported/Unsupported in WebUI;
- probe failure alone is not Unsupported.

### 4.2 Turn limit 8

Required:

```text
default max_turns = 150
valid range = 2..500
```

Expose in WebUI Agent settings. Preserve independent max tool calls, runtime timeout, context budget, cancellation, and no-progress detection.

### 4.3 Slow Telegram response despite fast model

Required:

- Custom Chat Completions/Responses implement real streaming;
- draft receives user-visible text deltas before upstream completion;
- no hidden reasoning is streamed;
- tool deltas are internal;
- final permanent response still sent once;
- pre-generation memory semantic work is removed from critical path;
- skill/memory learning after success runs post-delivery;
- deterministic verification handles straightforward cases without a second LLM call.

### 4.4 Wasteful one-tool/one-round-trip patterns

Required:

- multiple read-only top-level tool calls may run concurrently;
- mutating calls stay sequential;
- stable result ordering;
- new structured `termux_job` accepts bounded steps and returns one aggregated result;
- every substep passes canonical policy/executor/audit;
- root escalation impossible through `termux_job`;
- plan/script cache reuses verified workflow structure without caching stale dynamic outputs.

## 5. Runtime architecture

```text
                          TELEGRAM
                             │
                       inbound message
                             │
                             ▼
                    Xiao RunService/Core
                             │
          ┌──────────────────┼────────────────────┐
          │                  │                    │
          ▼                  ▼                    ▼
  Context + attachments   Agent Loop V2      Progress/stream
          │                  │                    │
          │            provider stream ───────────┼──→ Telegram draft
          │                  │                    │
          │              tool calls               │
          │                  ▼                    │
          │        ToolExecutionScheduler         │
          │           │              │            │
          │      parallel reads    sequential     │
          │           │             writes        │
          │           └──────┬───────┘            │
          │                  ▼                    │
          │             ToolResults               │
          │                  │                    │
          └──────────────────┴────→ provider ─────┘
                                      │
                               verified final
                                      │
                                      ▼
                          permanent Telegram reply
                                      │
                                      ▼
                              delivery acknowledgement
                                      │
                                      ▼
                         background memory/skill jobs
```

## 6. Agent settings

New defaults:

```toml
[agent]
max_turns = 150
max_tool_calls = 256
max_no_progress_repeats = 3
max_runtime_seconds = 1800
context_max_chars = 32000
summary_threshold_chars = 24000
tool_output_max_chars = 4096
max_parallel_readonly_tools = 8
max_execution_plan_steps = 32
provider_streaming = true
parallel_readonly_tools = true
execution_plan_enabled = true
plan_cache_enabled = true
background_learning = true
```

New runs snapshot settings at start. WebUI config updates do not mutate a running task mid-turn.

## 7. Capability model

For each exact Custom profile/model/protocol:

```text
vision_state      Supported|Unsupported|Unknown
file_state        Supported|Unsupported|Unknown
streaming_state   Supported|Unsupported|Unknown
override           Auto|ForceSupported|ForceUnsupported
probe_status       Unprobed|Completed|Indeterminate
probe_version
evidence source/timestamp
```

Resolution precedence:

`owner override > runtime confirmed evidence > explicit unsupported evidence > probe positive evidence > Unknown`.

Endpoint/protocol changes invalidate automatic evidence.

## 8. Streaming contract

Provider adapter emits canonical events:

```rust
Status
TextDelta
ToolCallDelta
Usage
Completed(ProviderTurn)
```

Telegram only receives safe status + user-visible TextDelta. It never gets reasoning deltas or raw tool JSON.

## 9. Fast foreground path

Forbidden foreground pattern:

```text
semantic memory call → main model → semantic verifier → semantic skill synthesis → final user response
```

Target:

```text
local preflight → main model stream → tools if needed → hard-evidence verification → final user response
                                                        │
                                                        └─ semantic verification only if ambiguous

post-delivery → memory/skill semantic jobs
```

## 10. Tool scheduler

Consecutive statically read-only calls run concurrently up to configured semaphore. Any mutation forms a barrier. Unknown defaults sequential. Return results in original call order.

## 11. `termux_job`

One provider tool call can execute a bounded list of structured Termux programs/argv. No arbitrary shell command string. No root escalation. Each substep is separately audited. Provider receives a bounded aggregated result.

## 12. Cache

- `PlanCache`: normalized structured job definitions keyed by content + runtime/environment version.
- `ScriptCache`: file-backed trusted/generated scripts only, content hashed, secret-free, never `bash -c`.
- `ToolResultCache`: separate and default Never unless tool declares cacheability. Never cache live RAM/process/network/git-state by default.

## 13. Memory/skills

- explicit owner request is visible in current context immediately;
- obvious local memory intent may update a pending overlay without provider call;
- durable semantic reconcile is queued;
- verified successful run generates learning job after final delivery;
- background worker is low-priority, bounded, idempotent, restart-safe;
- learning failure never changes already completed run to failed.

## 14. Context and no-progress

150 is an emergency ceiling. Add loop checkpoint compaction and result-aware repetition detection. Raw audits remain stored; provider context keeps only bounded relevant observations.

## 15. Termux/root policy

Termux non-root structured commands are routine ALLOW. Direct root escalation through Termux is hard DENY. Root operations use typed AndroidBroker. Privileged mutations ASK when YOLO off; session YOLO auto-grants ASK with audit; hard DENY remains absolute.

## 16. WebUI

Add `Agent` section for limits/performance settings and capability overrides under Custom model details. Show persisted/effective state, learning queue, and recent run latency.

## 17. Observability

Persist bounded timing events for preflight, provider TTFT, first text delta, tool groups, verification, final delivery, learning. WebUI Runs and CLI JSON expose them.

## 18. Telegram

Preserve simplified daily-use slash commands. Do not add an `/agent` or `/settings` Telegram command. Draft streams safe progress + accumulated visible answer; final remains permanent rich message.

## 19. Migration

Baseline schema 26. Add coherent migration for capability evidence/overrides, learning jobs, substep audit, and timings as required. Preserve all existing owner/session/memory/skill/profile data.

## 20. Release gates

Must pass:

- all automated test groups from `15_TEST_AND_ACCEPTANCE_MATRIX.md`;
- exact-head GitHub Actions Rust/WebUI/Android gates;
- rooted Android real-device Telegram test;
- real multimodal Custom image test;
- streaming response test;
- `/stop` cancellation during stream and termux job;
- long task exceeding old 8-turn cap;
- measured proof that background learning no longer delays final Telegram delivery.

Only after these pass should repository version/docs be promoted to 0.3.1.
