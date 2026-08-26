# 02 — Agent Loop V2

## Problem in baseline

The baseline loop uses `agent.max_turns = 8`, validates only `2..=32`, and executes provider turns until final answer / verification / tool budget / no-progress / runtime timeout. On real tasks, 8 provider iterations are too easy to exhaust.

Simply changing 8 to 150 is not enough. A 150-turn loop with the same orchestration would amplify token cost and latency.

## New runtime settings

Recommended defaults:

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

Validation ranges:

```text
max_turns                    2..500
max_tool_calls               1..512
max_no_progress_repeats      1..10
max_runtime_seconds          10..3600
max_parallel_readonly_tools  1..16
max_execution_plan_steps     1..64
```

## Loop model

```text
receive request
   ↓
local-only fast preflight
   ↓
build bounded context
   ↓
provider turn (stream)
   ├─ final text ──→ fast deterministic verification
   │                   ├─ verified → FINAL
   │                   ├─ blocked  → FINAL BLOCKED
   │                   └─ ambiguous/action evidence missing → continue
   │
   └─ tool calls
        ↓
      scheduler
        ↓
      ordered ToolResults
        ↓
      provider continuation
```

## Turn accounting

A **turn** is one upstream provider request that participates in the agent loop. Tool substeps inside `termux_job` do not consume provider turns; they consume tool/substep budgets.

Track independently:

- provider turns;
- top-level tool calls;
- execution-plan substeps;
- no-progress signatures;
- elapsed runtime;
- context size.

## Deterministic-first decision policy

Foreground calls to the same LLM for meta-decisions must be minimized.

1. If the provider is agent-capable, do not pre-classify every user request using a semantic provider call before the first main generation.
2. Use deterministic task classification first.
3. CompletionVerifier uses hard observable audit facts first.
4. Only ambiguous action completion may escalate to one bounded semantic verification call.
5. Informational final answers with no tool-side-effect requirement must not incur a separate semantic completion provider call.

## No-progress signatures

A no-progress signature includes bounded, redacted observable state:

```text
provider step kind
requested tool names
canonical arguments hash
result status
result semantic hash / bounded public summary
verification state
missing evidence category
```

Do not use raw secret-bearing outputs.

Early stop patterns:

- identical failed action repeated;
- ping-pong between two equivalent action signatures;
- same verification gap after materially identical tool observations;
- empty tool-call turns;
- provider repeatedly emits final answer that adds no new observable evidence.

## Why 150 is safe

150 is only the absolute ceiling. In normal operation:

- read-only parallelism reduces wall time;
- `termux_job` reduces provider round trips;
- no-progress detection ends bad loops after ~3 repeats;
- context compaction prevents unbounded transcript growth;
- runtime timeout remains a second ceiling;
- tool budget remains independent.
