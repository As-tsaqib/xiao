# 14 — Storage and Migration

## Schema strategy

Baseline schema is 26. v0.3.1 may use one new migration (recommended schema 27) containing the minimum durable changes needed for:

- capability runtime evidence/overrides;
- streaming capability state;
- background learning jobs;
- tool plan/substep audit if implemented in DB;
- run timing events/columns.

Do not create multiple partially dependent migrations if one coherent migration is easier to validate and roll back during development.

## Capability fields

Existing profile model records already contain tri-state fields and probe metadata. Extend rather than duplicate when practical:

```text
vision_override          auto|force_supported|force_unsupported
file_input_override      auto|force_supported|force_unsupported
streaming_state          supported|unsupported|unknown
streaming_override       auto|force_supported|force_unsupported (optional)
vision_runtime_confirmed_at
file_runtime_confirmed_at
streaming_observed_at
last_capability_error_kind
capability_epoch/version
```

Endpoint/protocol edit increments capability epoch or clears automatic observations.

## Learning jobs

Create idempotent queue table keyed by run id. On startup:

- pending stays pending;
- stale running older than lease becomes pending/retry;
- succeeded remains succeeded;
- failed may retry only under bounded policy/manual action.

## Tool plan audit

If `termux_job` is implemented, preserve substep observability either with:

- `tool_run_steps(parent_tool_run_id, ...)`, preferred; or
- child tool_run rows linked by `parent_tool_run_id`.

Provider sees one aggregated top-level ToolResult, but audit can still inspect each command.

## Run timings

Either add a compact `agent_run_events` table or timing columns. Event table is more extensible and avoids a wide agent_runs schema.

```text
agent_run_events
  id
  agent_run_id
  event_kind
  elapsed_ms
  metadata_json_bounded
  created_at
```

Metadata must be allowlisted and secret-free.

## Config migration

Old configs without new Agent fields receive v0.3.1 defaults. Existing explicit values are preserved, except old default `max_turns=8` should be migrated carefully:

- if field absent → 150;
- if explicitly set by owner → preserve even if 8;
- if the serialization path always materialized 8 and cannot distinguish default from explicit, document migration rule and prefer a one-time upgrade only when value equals the old known default and no owner-setting audit exists.

WebUI must show the effective value after migration.
