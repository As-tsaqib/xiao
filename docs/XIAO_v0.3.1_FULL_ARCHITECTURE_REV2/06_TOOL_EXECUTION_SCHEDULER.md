# 06 — Tool Execution Scheduler

## Purpose

Reduce tool latency without changing policy semantics.

## Grouping

Given provider calls in original order, produce execution groups:

```text
ReadOnly(A)
ReadOnly(B)
SideEffect(C)
ReadOnly(D)
ReadOnly(E)
Sensitive(F)
```

becomes:

```text
ParallelReadOnly[A,B]
Sequential[C]
ParallelReadOnly[D,E]
Sequential[F]
```

Do not use "all calls read-only or else everything sequential."

## Eligibility

Parallel candidate must be:
- `ToolRisk::ReadOnly`;
- no install/mutation requirement;
- no dependency on earlier result in the same batch;
- no approval requirement;
- cancellation-safe.

## Limits

```text
parallel_readonly_tools = true
max_parallel_readonly_tools = 8
```

Bound by semaphore.

## Result order

Execution can complete out of order internally, but results handed to provider continuation must match original call order.

## Audit

Every call records:
- call ID;
- tool;
- start/end;
- status;
- bounded output/error;
- group ID;
- original ordinal.

## Cancellation

`/stop` must:
- cancel pending group tasks;
- propagate token to active tools;
- mark unfinished rows `interrupted`;
- never leave a tool row `running` indefinitely.

## Approval barriers

Any ASK/approval-required call is a barrier. Parallel read-only work before it may complete; work after it must not start until the barrier is resolved or the plan is replanned.
