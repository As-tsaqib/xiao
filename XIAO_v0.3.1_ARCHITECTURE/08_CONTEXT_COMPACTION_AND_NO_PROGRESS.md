# 08 — Context Compaction and No-Progress Control

## Why this is required

A 150-turn ceiling is unsafe if every tool output and continuation is retained forever. v0.3.1 must bound provider context independently of turn count.

## Loop-local observable checkpoint

Maintain a compact run checkpoint derived only from observable state:

```text
<RUN_CHECKPOINT>
Goal: ...
Verified facts:
- ...
Successful actions:
- ...
Relevant failures:
- ...
Dependencies installed:
- ...
Artifacts:
- ...
Current missing evidence:
- ...
Provider turns used: ...
Tool calls used: ...
Remaining runtime: ...
</RUN_CHECKPOINT>
```

This replaces verbose stale tool transcripts when context pressure is high. Raw audit remains in SQLite.

## Compaction triggers

- context builder reaches `summary_threshold_chars`;
- structured fallback transcript approaches its byte cap;
- native continuation becomes too large or provider returns context overflow;
- more than N old tool results are retained and no longer relevant.

## What to preserve

Protect:

1. current explicit owner request;
2. current session summary;
3. relevant USER/MEMORY;
4. currently selected skills;
5. latest relevant tool results;
6. unresolved failures/blockers;
7. artifacts and verification evidence.

Drop/collapse:

- duplicate successful reads;
- obsolete failed attempts after a materially different successful strategy;
- repeated status messages;
- raw long stdout already summarized in audit.

## No-progress control

Recommended default `max_no_progress_repeats=3`.

Detect:

- same tool + same canonical args + same effective result;
- same failed signature;
- A/B ping-pong strategy;
- final-answer/verification cycle with unchanged missing evidence;
- empty calls or malformed protocol loops.

On no progress:

1. give the model one explicit bounded observation that repetition was detected;
2. require materially different strategy;
3. if threshold is reached, return Blocked with useful public evidence rather than burning all 150 turns.

## Context overflow recovery

A provider context-overflow error is not immediately fatal. Xiao may compact once and retry the same logical turn, provided the request has not already caused external side effects in that provider round trip.
