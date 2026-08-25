# 17 — Lessons from PicoClaw and ZeroClaw

These projects are references, not templates to copy wholesale.

## ZeroClaw patterns worth adopting

ZeroClaw's documented request lifecycle centers a shared turn engine where provider output streams, tool calls pass security gates, tools run, results return to the provider, and channels receive partial/final output. This is the right mental model for Xiao's fast path.

Useful pattern:

```text
channel → runtime → provider stream
                     ↓ tool call
                  security
                     ↓
                    tool
                     ↓
                  provider stream
                     ↓
                  channel partial/final
```

ZeroClaw also moved toward concurrent independent tool calls while preserving input result order. Its ecosystem also discusses explicit pipeline tools with bounded step counts and policy validation.

Important caution: pipeline orchestration must not bypass existing allow/deny policy. Xiao therefore requires every pipeline substep to re-enter canonical policy/executor boundaries.

References:

- https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/book/src/architecture/request-lifecycle.md
- https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/book/src/architecture/overview.md
- https://github.com/zeroclaw-labs/zeroclaw/issues/1043
- https://github.com/zeroclaw-labs/zeroclaw/issues/2152
- https://github.com/zeroclaw-labs/zeroclaw/issues/3683

## PicoClaw patterns worth adopting

PicoClaw exposes optional provider streaming and has been moving toward an event-driven agent loop with smart tool parallelism: consecutive read-only tools parallel, mutating tools sequential. This maps well to Xiao's Android/Termux workload.

PicoClaw also keeps configurable tool iteration limits rather than relying on one global hidden constant. Xiao should similarly thread the configured turn budget through every Telegram/CLI/WebUI path.

References:

- https://github.com/sipeed/picoclaw/blob/main/pkg/providers/types.go
- https://github.com/sipeed/picoclaw/blob/main/docs/guides/configuration.md
- https://github.com/sipeed/picoclaw/issues/1316

## What Xiao should NOT copy in v0.3.1

- subagents/subturns;
- extra provider families;
- broad plugin/microkernel work;
- autonomous background missions;
- large distributed architecture.

Xiao's advantage is that it can remain one single owner, one binary, one device runtime, and one Custom provider surface while borrowing the best execution mechanics.

## Xiao-specific synthesis

```text
ZeroClaw:  stream-first shared turn engine
PicoClaw:  smart read-only parallelism
Xiao:      single-owner Android runtime + Termux workshop + durable memory/skills

             ↓

Xiao v0.3.1:
stream-first
+ deterministic-first orchestration
+ 150-turn emergency ceiling
+ aggressive no-progress stop
+ parallel safe reads
+ structured Termux job
+ background learning
+ adaptive multimodal capability truth
```
