# 05 — Tool Execution Scheduler

## Baseline defect

When one provider turn returns multiple tool calls, the current AgentEngine processes them in a `for` loop and awaits each call before starting the next. Independent file reads or system inspections therefore waste wall-clock time.

## v0.3.1 scheduler

Extract top-level tool execution from the loop into a scheduler:

```rust
struct ToolExecutionScheduler { ... }

async fn execute_turn_calls(
    calls: Vec<ToolCall>,
    context: &ToolContext,
) -> Vec<ToolResult>;
```

## Execution classes

Use existing canonical tool metadata and add one conservative scheduling projection:

```rust
enum ToolExecutionClass {
    ReadOnlyParallelSafe,
    Sequential,
}
```

For v0.3.1:

- only statically declared read-only tools qualify for parallel execution;
- mutating/side-effect/privileged tools are sequential;
- unknown classification defaults to sequential;
- approval-required calls are sequential;
- tools that share a mutable exclusive resource may opt out of parallel execution.

## Grouping rule

Preserve provider call order while reducing wall time:

```text
calls: [read A, read B, mutate C, read D, read E]

scheduler groups:
  [read A, read B] parallel
  [mutate C]       sequential
  [read D, read E] parallel
```

Do not reorder a read across a mutation because the mutation may change what the later read observes.

## Result ordering

Even when execution is concurrent, the result vector returned to the provider must match the original call order and call IDs.

## Concurrency control

- default `max_parallel_readonly_tools=8`;
- semaphore scoped to an agent run;
- tool-specific timeouts preserved;
- parent cancellation token cloned to all parallel children;
- cancellation waits for cleanup and records interrupted status for each affected tool run.

## Audit

Each top-level call keeps its independent tool_run row and progress lifecycle. Parallel execution must not collapse auditability.

## Failure behavior

One read-only call failure does not cancel independent sibling reads unless the entire run is cancelled or runtime budget expires. Results are all returned to the model in stable order so it can choose a different strategy.

## Acceptance

Mock two 200 ms read-only tools. Parallel enabled must finish materially faster than sequential behavior while preserving result order. A mutating call inserted between them must enforce group barriers.
