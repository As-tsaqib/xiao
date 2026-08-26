# 09 — Agent Runtime Settings in WebUI

## New section

Add a first-class WebUI section:

```text
Agent
Loop, streaming, execution and latency controls
```

Do not hide these values under generic Runtime diagnostics.

## Fields

### Basic

- Maximum agent turns — default 150.
- Maximum tool calls — default 256.
- Runtime timeout — default 1800 seconds.
- No-progress repeat threshold — default 3.

### Performance

- Provider streaming — ON by default.
- Parallel read-only tools — ON by default.
- Maximum parallel read-only tools — default 8.
- Structured execution plan (`termux_job`) — ON by default.
- Plan cache — ON by default.
- Background learning — ON by default.

### Read-only diagnostics

- effective config generation/version;
- whether current daemon has loaded the settings;
- active run count;
- foreground semantic work count;
- background learning queue depth;
- current average/recent timing summary where available.

## Control-plane API

Use shared application service, not WebUI-owned business logic.

Suggested resource:

```text
GET  manager/agent
POST manager/agent
```

Example update body:

```json
{
  "action":"update",
  "max_turns":150,
  "max_tool_calls":256,
  "max_runtime_seconds":1800,
  "max_no_progress_repeats":3,
  "provider_streaming":true,
  "parallel_readonly_tools":true,
  "max_parallel_readonly_tools":8,
  "execution_plan_enabled":true,
  "plan_cache_enabled":true,
  "background_learning":true
}
```

## Hot reload

Baseline AgentEngine currently clones AgentConfig. v0.3.1 should introduce a shared settings snapshot service such as:

```rust
struct AgentSettingsStore {
    current: Arc<RwLock<AgentRuntimeSettings>>,
}
```

Rules:

- write config atomically;
- validate before commit;
- publish `AgentSettingsChanged`;
- new runs capture a settings snapshot at run start;
- active runs are not silently mutated mid-flight;
- WebUI reads both persisted and effective generation so stale reload is visible.

## UX validation

Inline input errors must explain allowed ranges. Reset-to-defaults is allowed but requires a normal confirm dialog in WebUI; no Telegram command is added.

## CLI parity

At minimum `xiao config show/check` must expose the values. If a dedicated setter is added, prefer structured CLI such as:

```text
xiao config agent show
xiao config agent set --max-turns 150
```

CLI parity is secondary to WebUI for this slice but must not create a second settings source.
