# 19 — Baseline Code Evidence

This file records the concrete code conditions seen at architecture baseline head `be8ccfb204e9ba512c6801f08af4ef2ef607b4e6`. It exists so an implementation agent can confirm whether a newer head has already changed any of them.

## Agent turn defaults

Path: `src/config/mod.rs`

Observed baseline behavior:

```rust
fn default_agent_max_turns() -> usize {
    8
}
```

Validation observed:

```rust
if !(2..=32).contains(&self.agent.max_turns) {
    return Err(...)
}
```

Agent loop in `src/agent/mod.rs` raises `agent turn limit ({}) reached before a final answer` when the configured ceiling is exhausted.

## Vision hard gate

Path: `src/agent/mod.rs`

Observed behavior after normalizing image attachments:

```rust
if !images.is_empty() && !provider_capabilities.vision {
    return Err(anyhow!(
        "selected provider/model does not declare vision capability..."
    ));
}
```

This gate occurs before the real user image reaches the provider.

## Capability projection

Path: `src/providers/mod.rs`

The Custom probe maps vision/file probe failures to `Unknown`, but persisted runtime booleans are true only when state is exactly Supported. Therefore Unknown can become an effective boolean false in current runtime gates.

The baseline vision probe renders a small custom bitmap OCR challenge and requires the model to reproduce an exact hidden challenge string. A failure to solve that task is not reliable proof that image transport is unsupported.

## Custom streaming disabled

Path: `src/providers/mod.rs`

Both Custom Chat Completions and Responses payload builders include:

```json
"stream": false
```

The adapter waits for the full JSON response before parsing final text/tool calls.

## Foreground semantic memory work

Path: `src/agent/mod.rs`

Before entering the main generation loop on a new user message, baseline code calls:

```rust
memory_evaluator.apply_explicit_async(...).await
```

When the selected Custom model supports structured output, the MemoryEvaluator can use the provider-backed SemanticEvaluator.

## Semantic worker timeout/repair

Path: `src/semantic/mod.rs`

Provider-backed semantic evaluator uses a bounded provider request and supports one repair attempt when the first schema response is malformed. This is correct for semantic reliability but costly when used on the foreground path for every message.

## Post-success learning awaited before AgentAnswer

Path: `src/agent/mod.rs`

After verified success, baseline code builds a LearningTrace and awaits:

```rust
learning.evaluate_async(principal, &trace).await
```

before finishing GenerationCompleted / returning AgentAnswer. LearningEvaluator can perform semantic reusability, skill synthesis/equivalence, and memory reconciliation.

## Sequential tool calls

Path: `src/agent/mod.rs`

For `ProviderStep::ToolCalls(calls)`, baseline executes a `for call in calls` loop and awaits each tool execution. Independent read-only calls therefore serialize wall time.

## Custom-only active runtime but legacy config remains

Path: `src/providers/mod.rs`

`build_providers()` registers only `custom` at baseline. Legacy Codex/Antigravity adapters remain for migration/history compatibility.

Path: `src/config/mod.rs`

`ProvidersConfig` still carries Codex/Antigravity fields and config validation still processes unused Antigravity URLs. v0.3.1 should isolate this legacy compatibility from normal Custom-only runtime validation.

## WebUI

Path: `webui/src/App.jsx`

Baseline WebUI has sections for Overview, Telegram, Custom AI, Sessions, Attachments, Runs, Memory, Skills, Tools, Security, Runtime, Diagnostics, and Logs. It does not expose `agent.max_turns` or a dedicated Agent runtime settings section.
