# Xiao v0.2.0 Architecture

## Runtime ownership

`xiaod` remains the only durable application owner. Xiao is a private personal
agent for one owner; retained principal IDs isolate compatibility/session
state, not tenants. `AppState` wires configuration, SQLite, the living
workspace, runtime probing/capabilities, sessions, authentication, providers,
`CommandCore`, health, and the event bus. Telegram, CLI/IPC, and WebUI remain
adapters; they do not own independent agent engines or durable state.

Startup create-loads `SOUL.md`, `USER.md`, `MEMORY.md`, and `AGENTS.md` under
the durable data root, then probes and atomically refreshes `ENVIRONMENT.md`.
Existing owner-edited files are never replaced by bootstrap defaults. Ordinary
task code has no SOUL write path; hard security rules remain compiled runtime
policy rather than workspace prose.

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

Completion moves through `running → verifying` into `completed`, `blocked`, or
`failed`. The verifier distinguishes `VerifiedSuccess`, `NotYetVerified`,
`Blocked`, and `Failed`. Information-only answers can complete without a tool.
Action tasks require an observed action and separate or typed postcondition
evidence; a nonempty final claim is insufficient. `NotYetVerified` is fed back
to the provider so it continues. Failed action signatures cannot be retried
unchanged indefinitely; turn/tool/no-progress/runtime bounds terminate loops.

Cancellation is checked at provider and tool boundaries. Startup changes any
persisted in-flight agent/tool runs to `interrupted`; uncertain side effects are
never automatically replayed.

## Tool registry and policy

`ToolRegistry` owns canonical `ToolSpec` identity/implementation lookup,
rejects duplicate names, resolves safe aliases, gates capabilities, advertises
only policy-eligible specs, applies timeouts, redacts output, and enforces an
output bound. Providers only translate those canonical definitions to wire
schemas. Runtime `ToolPolicy` evaluates risk and call arguments; a skill is
guidance and cannot grant a tool or bypass policy.

The v0.2.0 built-ins are:

- `context_stats`
- `memory_search`, `memory_set`, `memory_delete`
- `session_search`
- `skill_search`, `skill_view`
- `termux_terminal` (`terminal`/`exec` compatibility aliases)
- `android_xiao_status`, `android_xiao_restart`

The terminal accepts structured program/argv only, runs as the detected Termux
owner with a Termux-only PATH and controlled cwd/env, and bounds timeout,
cancellation, stdout, and stderr. A root daemon clears inherited groups, drops
UID/GID, and enables Linux/Android `no_new_privs` before exec. It rejects root escalation, model-supplied
shell command strings, unmanaged package mutation, and unsafe installer
pipelines. Clearly destructive, opaque shell-script, or credential-sensitive
calls require an exact one-shot approval. There is no generic root-shell tool.

When a trusted mapped binary is absent, `DependencyResolver` invokes only the
detected Termux `pkg`/`apt` backend with a normalized package, records the
install, re-probes the executable, and resumes the original call. Privileged
Android work is separate: `AndroidBroker` exposes only typed Xiao-service
inspection/restart; restart requires approval and accepts no command string.

## Long-term memory

`USER.md` and `MEMORY.md` are the inspectable editable active state. Managed
entries use stable semantic keys; atomic create/update/delete/merge/rekey
operations replace contradictions rather than accumulating append-only facts.
Manual owner edits are hash-reconciled back into SQLite. SQLite retains the
search index and `memory_history` audit; a one-time bridge exports legacy
SQLite-only active memories to files.

The `user` scope maps to owner profile/preferences; `agent` maps to durable
project/environment knowledge. The evaluator extracts general explicit
preferences/facts, compares related active entries, updates near-duplicates,
and supports explicit forget. Response style/language aliases exist for
compatibility but are not the evaluator's only supported subjects. Typed memory
tools use the same file-authoritative store.

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

`ContextEngine` replaces the old fixed last-N strategy. It assembles:

1. immutable Xiao system/security instructions and SOUL;
2. verified `RuntimeEnvironment`/`CapabilityRegistry` state;
3. USER and relevant MEMORY state plus AGENTS guidance;
4. selected relevant skill metadata/body;
5. durable session summaries and selective FTS5 prior excerpts;
6. newest conversation turns;
7. the current owner request.

Selection uses an approximate character budget. System/security instructions
and the current request are always retained, even when those protected fields
alone exceed the nominal budget. Older raw turns are trimmed first. When the
unsummarized middle exceeds the configured threshold, Xiao stores a bounded
extractive `session_summaries` row and retains recent turns intact; source
messages remain in SQLite.

## Skills and learning

A skill lives at `skills/<name>/SKILL.md` with YAML frontmatter requiring
`name` and `description`. Common optional community metadata is tolerated;
Xiao requirements live under namespaced metadata for binaries, capabilities,
and tools. Discovery/reconciliation indexes searchable fields in SQLite.
Search exposes eligibility metadata, full bodies load lazily, and trusted
missing Termux dependencies may be resolved before use. Ineligible skill
instructions never bypass ToolPolicy.

`LearningEvaluator` consumes the bounded observable trace after
`VerifiedSuccess`: goal, safe tool observations (including recovered failures),
final result, and verification evidence. It has no hidden-reasoning field.
Failed/cancelled/interrupted/unverified/trivial work creates no positive skill.
A reusable candidate generalizes when-to-use, prerequisites, procedure,
pitfalls, and verification; related intent is searched and merged before an
atomic SKILL write. `skill_history` retains the audit version.

## Storage and migrations

SQLite keeps WAL, foreign keys, a short mutex boundary, and graceful-shutdown
checkpointing. v0.2.0 migrations are additive and idempotent and retain every
v0.1.0 table. Schema versions add:

- version 6: `agent_runs`, `tool_runs` and lookup indexes;
- version 7: `memories`, `memory_history`, `memories_fts` and triggers;
- version 8: `messages_fts`, triggers, and `session_summaries`;
- version 9: `skills`, `skill_history`, `skills_fts` and triggers.
- version 10: `approvals`, `dependency_installs`, `environment_probes`,
  `workspace_file_index`, and `skill_file_index`.

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
databases, autonomous cron, browser automation, a plugin ecosystem, dynamic
native plugins, or unrestricted root execution.
