# 07 — Execution Plan and Cache

## `termux_job`

`termux_job` is a bounded structured workflow tool, not a shell escape hatch.

Each step contains:
- stable step ID;
- program;
- argv;
- relative cwd;
- continue_on_error.

No model-supplied opaque shell command string.

Max steps:
```text
max_execution_plan_steps = 32
```
Hard parser ceiling may remain 64 for compatibility.

## Substep policy

Each substep is policy checked.

If a substep requires approval:
- mark that substep `approval_required`;
- stop or follow `continue_on_error`;
- instruct runtime/provider to issue an exact separate one-shot call if approval is needed.

No approval is silently bypassed inside a batch.

## PlanCache — production requirement

The cache must be used by runtime, not only defined in `cache.rs`.

Cacheable:
- normalized safe structured execution plans;
- schema-versioned;
- environment-fingerprinted;
- secret-free.

Not cacheable:
- dynamic tool outputs;
- live process state;
- memory/session search results;
- secret-bearing arguments;
- root/privileged plans.

Runtime flow:

```text
normalized reusable plan
   ↓
cache lookup
 ├─ valid hit → revalidate environment/policy → reuse
 └─ miss      → create/validate → insert
```

Telemetry:
- `plan_cache_hit`
- `plan_cache_miss`
- `plan_cache_rejected`
- `plan_cache_invalidated`

## ScriptCache

A reusable script requires a file-backed manifest:

- path inside Xiao-controlled workspace/cache;
- trusted interpreter;
- SHA256 content hash;
- provenance/source;
- environment fingerprint;
- no embedded secret material;
- no root escalation;
- policy re-check before execution.

Unknown arbitrary `.sh` supplied by model/remote content is not equivalent to a trusted cached script.

Telemetry:
- `script_cache_hit`
- `script_cache_miss`
- `script_cache_hash_mismatch`
- `script_cache_policy_reject`

## Cache invalidation

Invalidate on:
- schema change;
- relevant runtime environment fingerprint change;
- interpreter mismatch;
- script hash change;
- policy version change;
- endpoint/profile trust change where provider-specific plans are involved.
