# Xiao v0.3.1 Architecture

## v0.3.1 control-plane unification

Telegram, CLI, and KernelSU WebUI are adapters over the same `xiaod` application services. One stable `owner_user_id` is the authorization identity; chat allowlists restrict location only. CLI sessions remain independent unless explicitly targeted; provider/session, memory, skills, approvals, attachments, Custom capability probing/editing, scanned-PDF processing, and Doctor diagnostics share the same control-plane semantics. Secrets remain write-only/masked.

## Runtime ownership

`xiaod` remains the only durable application owner. Xiao is a private personal
agent for one stable `OwnerIdentity`; retained principal IDs are migration
compatibility keys, not tenants. Owner-global USER/MEMORY state, skills,
provider accounts/profiles, credentials, and recall are separated from
`TelegramScope` (`chat_id + message_thread_id`) and session-specific model,
YOLO, attachments, and active-run state. `AppState` wires configuration,
SQLite, the living workspace, runtime/capabilities, sessions, authentication,
providers, attachments, `CommandCore`, health, and the event bus. Telegram,
CLI/IPC, and WebUI remain adapters and never become independent durable owners.

Startup create-loads `SOUL.md`, `USER.md`, `MEMORY.md`, and `AGENTS.md` under
the durable data root, then probes and atomically refreshes `ENVIRONMENT.md`.
Existing owner-edited files are never replaced by bootstrap defaults. Ordinary
task code has no SOUL write path; hard security rules remain compiled runtime
policy rather than workspace prose.

`CommandCore` is the semantic convergence point for frontend commands. A chat
request enters the owner-scoped `AgentEngine`, captures its target session,
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
they do not discover tools, apply policy, or execute tools. Every provider/model
reports an explicit `ToolProtocol`: `Native`, `StructuredJsonFallback`, or
`ChatOnly`. Codex uses Responses function-call continuation. Antigravity maps
canonical tools/results through Gemini `functionDeclarations`, `functionCall`,
and `functionResponse`. Each Custom model is probed for a synthetic native
function call, then a strict JSON envelope; an unprobed/unsupported model is
explicitly `ChatOnly`. Xiao never silently removes tools and presents an action
model as an equivalent agent. Custom configuration is stored in owner-global,
independently credentialed profiles; requests use only the selected profile's
endpoint, credential reference, and headers. Structured fallback retains a
bounded normalized transcript so Tool A/result A can lead to Tool B/result B
and then a final response without losing relevant prior observations.

`SemanticEvaluator` is a separate no-tools boundary used for task intent,
completion interpretation, memory lifecycle decisions, trace learning, skill
synthesis, and skill equivalence. Inputs/outputs are bounded and redacted,
require schema-conforming JSON, permit one format repair, and fail
conservatively. Semantic output cannot grant a tool, override `ToolPolicy`, or
turn model prose into action evidence. Provider-backed decisions use one
reusable bounded worker runtime with concurrency limits, timeout, and
cancellation so evaluations cannot create a fresh runtime/thread per request or
accumulate without bounds.

Completion moves through `running → verifying` into `completed`, `blocked`, or
`failed`. The verifier distinguishes `VerifiedSuccess`, `NotYetVerified`,
`Blocked`, and `Failed`. Information-only answers can complete without a tool.
Action tasks require an observed action and separate or typed postcondition
evidence; a nonempty final claim is insufficient. `NotYetVerified` is fed back
to the provider so it continues. Failed action signatures cannot be retried
unchanged indefinitely; turn/tool/no-progress/runtime bounds terminate loops.
The next turn receives a bounded runtime-owned `RUN_OBSERVATIONS` block with
successful/failed actions, installs, artifacts, missing evidence, attempt count,
and remaining budgets—never private chain-of-thought.

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

The v0.3.1 built-ins are:

- `context_stats`
- `memory_search`, `memory_set`, `memory_delete`
- `session_search`
- `skill_search`, `skill_view`
- `termux_terminal` (`terminal`/`exec` compatibility aliases)
- `termux_job`
- `pdf_create`
- `android_xiao_status`, `android_xiao_restart`

The terminal accepts structured program/argv only, runs as the detected Termux
owner with a Termux-only PATH and controlled cwd/env, and bounds timeout,
cancellation, stdout, and stderr. A root daemon clears inherited groups, drops
UID/GID, and enables Linux/Android `no_new_privs` before exec. It rejects root escalation, model-supplied
shell command strings, unmanaged package mutation, and unsafe installer
pipelines. Clearly destructive, opaque shell-script, or credential-sensitive
calls require an exact one-shot approval bound to owner, session, agent run,
tool call, tool name, argument hash, and expiry. Consumption is atomic and
one-time. There is no generic root-shell tool.

When a binary is absent, `DependencyResolver` first uses the known trusted
mapping and can then query trusted Termux repository metadata. A candidate must
have a normalized package name, trusted source, and exact/provided-binary
relationship before the detected `pkg`/`apt` backend can install it. Every
install records source/validation metadata, re-probes the executable, refreshes
capability state, and resumes the original call. Language ecosystem installers
are not auto-approved and arbitrary remote installer scripts remain forbidden.
Privileged Android work is separate: `AndroidBroker` exposes only typed
Xiao-service inspection/restart; restart requires approval and accepts no
command string.

## Long-term memory

`USER.md` and `MEMORY.md` are the inspectable editable active state. Managed
entries use stable semantic keys; atomic create/update/delete/merge/rekey
operations replace contradictions rather than accumulating append-only facts.
Manual owner edits are hash-reconciled back into SQLite. SQLite retains the
search index and `memory_history` audit; a one-time bridge exports legacy
SQLite-only active memories to files.

The `user` scope maps to owner profile/preferences; `agent` maps to durable
project/environment knowledge. The semantic evaluator retrieves related
current state and chooses `NONE`/`CREATE`/`UPDATE`/`DELETE`/`MERGE`/`REKEY`.
Explicit owner changes replace old semantic state, near-duplicates do not
accumulate, and explicit forget removes/deactivates state. Deterministic
extraction is a conservative fallback, not the primary intelligence. Typed
memory tools use the same file-authoritative store.

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

## Attachments, vision, and documents

Telegram photo/document updates retain their `TelegramScope`, resolve the
active Xiao session, and download through bounded Bot API methods into private
controlled paths. Pre/post-download size limits, per-session quota, sanitized
names, content sniffing, image decode/dimension checks, and SHA-256 metadata are
applied before an attachment becomes ready. Telegram/provider wire formats do
not leak into the normalized attachment and multimodal content model.

Vision content is sent only when the selected provider/model has verified
vision capability. A non-vision or unknown model receives no image bytes and
returns a factual capability/model-switch blocker. Provider adapters serialize
the normalized image/caption into their own protocol and enforce their size
limits.

TXT, Markdown, source/plain text, JSON, CSV, embedded-text PDF, and DOCX are
extracted without executing document macros or scripts. An image-only PDF is
marked `needs_ocr` instead of treating empty output as success. Normalized text
is chunked into SQLite FTS5; ContextEngine retrieves bounded relevant chunks
rather than loading an entire large document. Recent session attachment
references allow phrases such as “file tadi” or “dokumen kedua” to resolve
without crossing session ownership.

## Skills and learning

A skill lives at `skills/<name>/SKILL.md` with YAML frontmatter requiring
`name` and `description`. Common optional community metadata is tolerated;
Xiao requirements live under namespaced metadata for binaries, capabilities,
and tools. Discovery/reconciliation indexes searchable fields in SQLite.
Search exposes eligibility metadata, full bodies load lazily, and trusted
missing Termux dependencies may be resolved before use. Ineligible skill
instructions never bypass ToolPolicy.

`LearningEvaluator` consumes the bounded sanitized observable trace after
`VerifiedSuccess`: goal, safe tool observations (including recovered failures),
final result, and verification evidence. It has no hidden-reasoning field.
Failed/cancelled/interrupted/unverified/trivial work creates no positive skill.
A reusable candidate describes concrete successful operations as when-to-use,
prerequisites, procedure, recovered pitfalls, and observable verification.
Deduplication performs canonical lookup, FTS/lexical retrieval, then semantic
equivalence to create/update/merge/leave unchanged before an atomic SKILL
write. Learned/imported source kinds are inspectable; skills may be disabled,
and only learned owner-created skills may be deleted. `skill_history` retains
the audit version.

## Telegram topic scope and command UX

`TelegramScope` is `chat_id + message_thread_id`; owner ID remains authorization,
not a conversation namespace. Main/side activation, menus, callbacks, wizard
input, drafts, replies, and documents preserve the scope. Legacy Telegram
sessions migrate to the default non-topic scope. YOLO is persisted per Xiao
session, defaults off for new/topic/side sessions, converts only `ASK` to an
audited auto-approval, and never changes `DENY`.

`TelegramCommandRegistry` is the single source for parsing aliases, `/help`,
ordering, and Bot API `setMyCommands`. Public commands are `/start`, `/help`,
`/login`, `/model`, `/new`, `/sessions`, `/btw`, `/status`, `/context`,
`/cancel`, `/retry`, `/yolo`, `/memory`, `/skills`, `/tools`, `/doctor`, and
`/approvals`. `/session` and `/stop` are hidden aliases; `/provider`,
`/settings`, `/usage`, `/env`, `/about`, and `/logout` have no public route.
`/model` is the unified AI-management hub for the current provider/profile,
account, and model plus paginated account/profile/model management.

The Custom `/login` wizard is expiring and owner/chat/topic/menu-bound. Each
endpoint/key/alias phase retires the prior keyboard and sends a new prompt. It
keeps endpoint/key/model blobs out of callback payloads, validates the endpoint,
accepts an optional zeroized API key, best-effort deletes the owner's key
message, scrubs the recognized credential payload from Xiao's durable Telegram
inbox, discovers models five per page, probes the selected model protocol, and
requires confirmation before secure persistence. Discovery errors expose a
concrete reason plus phase-aware Retry/Edit Endpoint/Back/Close recovery
actions. A selected profile is authoritative independently of the legacy
singleton flag; failed cross-table commits restore the prior session and remove
partial profile state.

Before owner-facing memory or skill list/search operations, canonical living
files are reconciled and stale skill indexes are rescanned. The shared
five-items-per-page paginator is also used for sessions, accounts, profiles,
models, approvals, memory, and skills; callbacks remain scoped, revisioned, and
expiry-checked.

## Xiao Manager and diagnostics

The KernelSU WebUI is a management console, never a second application server:
`WebUI → authenticated loopback admin API → xiaod → typed managers/stores`.
Dashboard, Providers, Runtime, Sessions, Tasks, Memory, Skills, Tools, Security,
Diagnostics, and Logs expose bounded observable state. Mutations use named
admin actions; the API has no generic SQL, filesystem-write, secret-read, or
root-shell endpoint. Secrets are masked/write-only and surfaced logs/exports
are redacted and bounded.

`/doctor` and the Diagnostics page execute independent read-only probes for
Telegram, DB transaction/schema, identity, memory, skills, Termux,
CapabilityRegistry, Android broker, selected provider/auth/model and Custom
reachability, attachment store/FTS, session FTS, and admin backend. Each result
uses PASS/WARN/FAIL/SKIPPED with concise evidence; one healthy subsystem cannot
mask another subsystem's failure.

## Storage and migrations

SQLite keeps WAL, foreign keys, a short mutex boundary, and graceful-shutdown
checkpointing. v0.2.7 migrations are additive and idempotent and retain every
v0.1.0 table. Schema versions add:

- version 6: `agent_runs`, `tool_runs` and lookup indexes;
- version 7: `memories`, `memory_history`, `memories_fts` and triggers;
- version 8: `messages_fts`, triggers, and `session_summaries`;
- version 9: `skills`, `skill_history`, `skills_fts` and triggers.
- version 10: `approvals`, `dependency_installs`, `environment_probes`,
  `workspace_file_index`, and `skill_file_index`.
- version 11: Telegram topic/session bindings and active scope state;
  per-session YOLO/tool approval audit fields; provider capability metadata;
  skill prerequisites; dependency source/validation metadata.
- version 12: learned/imported `skills.source_kind` and owner-controlled
  `skills.enabled`, including legacy source classification.
- version 13: stable `owners` plus legacy-principal migration mapping.
- versions 14–15: exact approval binding fields/indexes, owner-bound provider
  accounts, isolated Custom profiles, and per-profile model/capability rows.
- version 16: attachment metadata, extracted chunks, FTS5 index, triggers, and
  backfill.

Migration tests cover a fresh database, hand-built v0.1.0 and representative
v0.2.5 upgrades, WebUI-first `owner:local` claiming, repeated migrations,
transactional/idempotent rekeying, history/session/run/profile preservation,
FTS consistency, and restart quarantine of in-flight runs.

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

v0.2.7 deliberately does not add MCP, remote/device nodes, subagents, vector
databases, autonomous cron, browser automation, a plugin ecosystem, dynamic
native plugins, or unrestricted root execution.
