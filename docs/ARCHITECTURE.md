# Xiao v0.2.0 Architecture

## Runtime ownership

`xiaod` remains the only durable application owner. `AppState` wires
configuration, SQLite storage, sessions, authentication, providers,
`CommandCore`, health state, and the event bus. Telegram, CLI/IPC, and WebUI
remain adapters; they do not own independent agent engines or durable state.

`CommandCore` is the semantic convergence point for frontend commands. A chat
request enters the principal-scoped `AgentEngine`, captures its target session,
and retains that target even if the frontend switches sessions concurrently.
Only safe typed `AgentEvent` progress crosses the frontend boundary. Hidden
provider reasoning is neither modeled nor persisted.

## Agent and provider loop

Each generation creates an `agent_runs` row before context/provider setup. The
runtime builds bounded context, resolves the provider/model, then performs at
most `[agent].max_turns` provider turns. A provider may return a final answer or
canonical `ToolCall` values. Every tool call is persisted before execution,
marked running, and finished as `succeeded`, `failed`, `denied`, or
`interrupted`. Tool errors become provider observations instead of daemon
crashes.

The provider contract receives canonical `ToolSpec` values selected by the
agent runtime. Providers only translate them to wire format and parse calls;
they do not discover tools, apply policy, or execute tools. Codex declares
tool-continuation capability. Providers without that capability receive no
advertised tools, and an unexpected tool-call response is an explicit error.

Completion moves through `running → verifying → completed`. A nonempty final
answer and resolved observable tool results are required. Failed, denied, or
interrupted tool work must be followed by a later successful recovery of the
same tool to verify completion. Information-only answers can complete without
tools, but are not considered meaningful reusable procedures.

Cancellation is checked at provider and tool boundaries. Startup changes any
persisted in-flight agent/tool runs to `interrupted`; uncertain side effects are
never automatically replayed.

## Tool registry and policy

`ToolRegistry` owns canonical identity and implementation lookup, rejects
duplicate names, advertises only policy-allowed specs, applies per-tool
timeouts, redacts output, and enforces a configured output bound.
`ToolPolicy` permits read-only tools and only the explicitly approved
`memory_set`/`memory_delete` side effects. Sensitive, destructive, privileged,
and unapproved side-effect tools are denied.

The v0.2.0 built-ins are:

- `context_stats`
- `memory_search`, `memory_set`, `memory_delete`
- `session_search`
- `skill_search`, `skill_view`

There is no command/process/root-shell tool. Skills and memories are data, not
capabilities, and cannot register tools or bypass policy.

## Long-term memory

Memory is editable current state, keyed by
`(owner_principal, scope, category, key)`. SQLite enforces that uniqueness.
`MemoryStore::upsert` updates an existing canonical row, while
`memory_history` records create/update/delete audit events separately. Only
active `memories` rows enter normal context.

The `user` scope holds durable user preferences/facts; `agent` holds durable
project knowledge learned for that principal. Canonical aliases collapse
common overlapping keys such as answer style/verbosity into
`preference.response_style`. Deterministic explicit handling recognizes stable
preference changes and bounded `remember … X is Y` facts before the provider is
called. Explicit forget removes matching active state. Arbitrary mutations
remain available through typed memory tools.

Memory values and sensitive identities are bounded and credential-screened.
Structured tool arguments are recursively redacted before audit persistence.
Implicit learning is deliberately conservative and runs only after verified
completion.

## Retrieval and context

Raw messages remain authoritative and are never deleted by context
compression. `messages_fts` is maintained by SQLite triggers and joined back to
owned sessions for every search, so results cannot cross principal boundaries.
`session_search` bounds both result count and content size and redacts likely
credential material.

`ContextEngine` replaces the old row-count truncation strategy. It assembles:

1. immutable Xiao system/security instructions;
2. active user memory;
3. active agent/project memory;
4. selected relevant skills;
5. durable session summaries;
6. retrieved prior history when the request refers to earlier work;
7. newest conversation turns;
8. the current user request.

Selection uses an approximate character budget. System/security instructions
and the current request are always retained, even when those protected fields
alone exceed the nominal budget. Older raw turns are trimmed first. When the
unsummarized middle exceeds the configured threshold, Xiao stores a bounded
extractive `session_summaries` row and retains recent turns intact; source
messages remain in SQLite.

## Skills and learning

A skill is principal-scoped procedural memory containing `name`, `summary`,
`when_to_use`, `procedure`, `pitfalls`, and `verification`. SQLite FTS indexes
searchable fields. Context searches skill summaries from the current request
and progressively discloses only selected full skills rather than injecting
the entire registry.

`LearningEvaluator` accepts a bounded observable trace: goal, safe tool
observations, final observable result, and verification evidence. It has no
hidden-reasoning field. Learning is skipped for failed, cancelled, interrupted,
unverified, trivial, or non-reusable work. A candidate is searched against
existing skill intent before creation. Canonical token aliases and overlap
scoring merge near-synonyms such as `fix-xiao-service-v2` into
`diagnose-xiao-service`; procedure, pitfalls, and verification are updated in
one canonical row, with `skill_history` retaining the audit version.

## Storage and migrations

SQLite keeps WAL, foreign keys, a short mutex boundary, and graceful-shutdown
checkpointing. v0.2.0 migrations are additive and idempotent and retain every
v0.1.0 table. Schema versions add:

- version 6: `agent_runs`, `tool_runs` and lookup indexes;
- version 7: `memories`, `memory_history`, `memories_fts` and triggers;
- version 8: `messages_fts`, triggers, and `session_summaries`;
- version 9: `skills`, `skill_history`, `skills_fts` and triggers.

Migration tests cover a fresh database, a hand-built v0.1.0 database upgrade,
repeated `migrate()` calls, FTS backfill consistency, and restart quarantine of
in-flight runs.

## Preserved v0.1.0 invariants

Telegram still uses durable long polling. Inbox acceptance and offset advance
remain atomic; accepted-but-unclaimed updates replay, while claimed uncertain
updates are quarantined. ACL is checked before pending-input capture, command
parsing, agent dispatch, or provider work. Per-principal cancellation and
MAIN/SIDE ownership/isolation remain unchanged.

IPC still rejects non-loopback binds, authenticates with constant-time checks,
and separates limited client from root-admin credentials. Credentials remain
in `SecretStore`; snapshot surfaces presence only. The managed Termux wrapper
elevates only fixed module binaries. KernelSU/WebUI lifecycle shell paths are
fixed administrative code and are never reachable from model tool calls.

v0.2.0 deliberately does not add MCP, remote/device nodes, subagents, vector
databases, autonomous cron, browser automation, plugins, or generic process
execution.
