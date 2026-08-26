# 07 — Memory and Skills Background Pipeline

## Baseline latency problem

The current path can perform provider-backed semantic memory evaluation before the main agent generation, semantic completion work after generation, and skill/memory learning before the AgentAnswer returns. Even with a fast model, those extra LLM round trips can dominate user-visible latency.

## Principle

**Current-turn understanding is not dependent on immediately writing durable memory.** The user's current message is already in the current context. Durable reconciliation may happen asynchronously as long as immediate owner intent is not lost.

## Split memory handling

### Foreground: local-only fast path

Create a deterministic `FastMemoryIntent` layer that:

- never calls the provider;
- recognizes explicit forget/change/remember patterns when confidence is high;
- creates a small in-memory/session pending overlay for immediately obvious current-owner state;
- records a durable reconciliation job.

If the instruction is ambiguous, do not block the response. Queue semantic reconciliation.

### Background: durable semantic reconciliation

A background worker:

- evaluates owner statement against canonical USER/MEMORY;
- performs NONE/CREATE/UPDATE/DELETE/MERGE/REKEY;
- remains idempotent by source message/run id;
- retries bounded transient failures;
- never stores secrets.

## Learning job queue

Add durable jobs keyed by completed run id.

```text
learning_jobs
  id
  owner_id
  run_id UNIQUE
  status = pending|running|succeeded|failed
  attempts
  not_before
  last_error_redacted
  created_at
  updated_at
```

A daemon restart resumes stale pending/running jobs safely.

## Delivery ordering

For Telegram, background semantic learning must not compete with the foreground answer before delivery.

Preferred lifecycle:

```text
AgentEngine produces verified final answer
  ↓
Telegram permanent final send succeeds
  ↓
RunService delivery acknowledgement
  ↓
enqueue / release learning job
```

If the architecture cannot yet obtain an exact delivery ACK from every frontend, use a durable `not_before` delay and a foreground-priority semantic scheduler so background jobs cannot starve a live generation.

## Background priority

Use a bounded worker, recommended concurrency 1 by default on Android. Foreground generation always wins over post-delivery semantic work for the same provider/profile.

## Completion verification

Do not move security-critical verification to background. Instead optimize it:

1. deterministic hard-evidence check first;
2. informational/no-action answers complete immediately;
3. inspection with successful read-only observations may complete deterministically;
4. action tasks with explicit independent verification may complete deterministically;
5. only genuinely ambiguous cases use one semantic completion call.

## Skill learning

Skill synthesis/dedup is always post-delivery in v0.3.1.

Successful run trace remains:

- goal;
- tool/substep observations;
- dependencies;
- artifacts;
- corrections/failures;
- final result;
- verification evidence.

No hidden reasoning is introduced.

## Failure semantics

A learning job can fail independently while the user-visible run remains `completed`. WebUI may show a low-priority warning/retry status, but Telegram answer is not retroactively changed.
