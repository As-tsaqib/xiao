# 02 — Agent Loop V2

## Desired foreground path

```text
owner request
   ↓
load session/config snapshot
   ↓
build bounded context
   ↓
LOCAL deterministic task classification
   ↓
build tool exposure
   ↓
MAIN provider request immediately
   ↓
stream visible events
   ↓
tool calls?
  ├─ no → deterministic/evidence-first verify → final
  └─ yes
       ↓
     schedule tool groups
       ↓
     execute / observe / append tool results
       ↓
     provider continuation
       ↓
     verify
       ↓
     retry only with measurable progress
       ↓
     final
```

## Deterministic task classification

Pre-provider classification must be local. It should classify obvious cases:

- Informational
- Inspection
- Action
- Modification
- Installation
- Verification
- Mixed

If ambiguous, choose the safer runtime behavior locally. Do not perform an LLM classification call before the main provider request.

For ambiguity between informational and action, prefer exposing only safe read-only tools unless the wording or subsequent provider call clearly requests action.

## Tool exposure

For informational/code-example prompts:
- expose read-only context/search tools only;
- exclude `termux_terminal`, `termux_job`, Android mutation, memory mutation, and artifact side effects unless explicitly requested.

For action-like prompts:
- expose canonical tools allowed by provider capability and runtime policy.

## Turn budget

Recommended defaults:

```text
max_turns = 150
max_tool_calls = 256
max_runtime_seconds = 1800
max_no_progress_repeats = 3
```

The configuration is snapshotted at run start.

## No-progress detection

Do not block because the same command name appeared again.

Progress signature should include at least:

```text
tool name
normalized arguments
tool status
bounded output/error digest
relevant artifact/state digest
verification state
```

A repeated command after changed observable state is progress.

Examples:

```text
edit file → test fails A → edit same file → test fails B
```

is progress.

```text
same command + same args + same result + same state
repeated N times
```

is no progress.

## Completion

### Informational
No semantic verifier call by default. A coherent final answer is sufficient unless a user explicitly required live verification.

### Inspection
Successful read-only observation or direct image inspection can satisfy the task. If evidence is ambiguous, one bounded semantic completion call may interpret existing evidence, but may not invent new facts.

### Action
Require:
- at least one successful side effect;
- successful verification evidence after the side effect, or a self-verifying typed operation whose contract explicitly includes verification.

### Blocked
Use Blocked only for real barriers such as:
- required approval denied/expired;
- unsupported hard capability;
- exhausted runtime/turn budget with no progress;
- missing dependency that policy cannot install;
- unavailable required external service after bounded retries.

Do not turn a valid direct image answer into Blocked merely because no side-effect tool ran.

## Verification retry

When verification says NotYetVerified:
- append a bounded RUN_OBSERVATIONS summary;
- ask the same main provider to continue;
- do not create a second hidden planner;
- stop on no-progress threshold or runtime budget.
