# xiao v0.1.0 Architecture

## Ownership boundaries

`xiaod` owns all durable application state. `AppState` wires an `AppConfig`, `Storage`, `SessionManager`, `AuthManager`, `ProviderRegistry`, `CommandCore`, health state, and internal event bus. Frontends only translate transport-specific input into semantic commands and translate semantic `View`/`CommandResult` output back to their transport.

The Command Core is the convergence point. Telegram messages, inline keyboard actions, and Termux requests all call the same semantic core. WebUI is intentionally an administrative adapter: it uses authenticated local admin endpoints for configuration/status and never becomes a second session/agent engine.

## Telegram

`TelegramAdapter` uses `getUpdates` long polling with a durable SQLite inbox and reconnect backoff. Acceptance of an update and advancement of the Telegram offset happen in one transaction. Accepted-but-unclaimed rows are replayed after restart. Once a row is claimed (`processing`), a crash or handler error is an uncertainty boundary: the row is quarantined as `interrupted`/`failed` instead of being automatically replayed, preventing duplicated destructive semantic commands. ACL is checked before any pending-input capture, command parsing, agent dispatch, or provider call.

The poller never waits for long-running generation. Accepted updates are dispatched into asynchronous tasks; independent principals can make progress concurrently. Non-generation semantic commands for one Telegram principal pass through a per-principal mutation lane, while provider generation deliberately runs outside that lane. Agent generation itself is cancellable per principal, and `/stop` resolves/cancels the active `CancellationToken` without waiting for the original provider request to finish. A generation captures its immutable target session ID before provider execution, so a concurrent session switch cannot redirect the final write.

Interactive views are stored as short-lived in-memory `MenuSession` records containing owner, chat/message IDs, current `View`, Back history, revision, expiry, and optional next-message capture. Callback data is `m:<10-char-id>:<revision-hex>:<index-hex>`, safely below Telegram's 64-byte limit. The callback spinner is acknowledged before waiting for the per-menu lock. Owner and revision are rechecked before mutation.

Navigation is edit-first: rich edit, plain edit, rich replacement, then plain replacement. On replacement, the previous keyboard is retired. Persistent session/provider/model/account state remains in the daemon even when the ephemeral menu expires.

Agent progress uses one `sendRichMessageDraft` draft ID per update and is throttled to roughly 750 ms. A conservative 20-second heartbeat refreshes the same draft while generation is active, below the current Telegram draft-preview lifetime, so silent provider/tool periods do not make progress disappear. Only safe status/tool summaries from the typed `AgentEvent` channel are rendered. The active semantic activity (`Thinking`, analysis, search, fetch, tool, code, media, or writing) is represented by an animated custom emoji from Telegram's official AI Actions set inside the native draft-only `thinking` block. Completed meaningful work becomes a quiet check-mark row; transient thinking/writing states are replaced instead of accumulating. The emoji itself animates client-side, so xiao does not consume the Bot API rate limit with manual animation frames. Final output is parsed into the transport-neutral Presentation AST and sent as one or more separate persistent Rich Messages; progress blocks never enter final history.

## Sessions and `/btw`

Main sessions are durable rows in SQLite and every row has an immutable `owner_principal`. Telegram uses the principal form `telegram:<chat_id>:<user_id>`, which intentionally isolates two users even when they share an authorized group chat. Ownership is enforced in storage queries and mutations (not merely UI filtering) for list, switch, detail/history, rename, archive, messages, context/retry, and side-session access. `/new` creates and activates another main session for the same principal without deleting older sessions. The session manager lists five rows per page. Rename's “next message” is a frontend UI capture only; the eventual rename still executes as `/session rename <id> <name>` through Command Core.

SIDE mode creates a child side-session bound to one main session and inherits that main session's owner. Agent context is main history plus side history, but user/assistant writes target only the active side session. Toggling `/btw` while in SIDE returns to MAIN, so nesting is impossible. The final renderer visibly labels SIDE replies.

## Providers and authentication

Provider-specific HTTP/auth details live behind `Provider` and `AuthManager` boundaries. Sessions bind `provider`, `account_id`, and `model`. The semantic `UseAccount` operation resolves the target account/provider/default valid model first and then commits provider + account + model as one SQLite transaction; failure cannot leave a half-switched session.

The agent loop is typed: a provider can return `ToolCalls`, `ToolRouter` applies the v0.1.0 allowlist/policy and timeout/output limits, results are emitted as safe `AgentEvent`s and returned to the provider for continuation. v0.1.0 intentionally exposes only bounded internal tools (for example context statistics); there is no model-controlled arbitrary process or root-shell executor. Provider streaming consumes SSE incrementally rather than buffering an entire response before emitting progress.

Credentials are per account and stored separately in SecretStore. Refresh uses per-account async locks to avoid concurrent refresh races. Codex browser login follows CLIProxyAPI's Authorization Code + PKCE contract and binds its localhost callback only for the active transaction. Antigravity follows CLIProxyAPI's installed-app OAuth flow, userinfo lookup, and `loadCodeAssist`/`onboardUser` project bootstrap. Its localhost callback is likewise transaction-scoped. Both adapters can change endpoint/auth details without changing Telegram/Termux or Command Core.

## Storage

SQLite enables WAL and foreign keys. Versioned, additive migrations create/upgrade sessions (including ownership), messages, frontend state, provider accounts, settings, Telegram durable inbox/offset state, and audit-event storage without destroying existing user data. Full message history persists; effective provider context is bounded without deleting history. Synchronous `rusqlite` work is kept behind a short mutex and enters a Tokio blocking boundary on the multithread runtime so database work does not monopolize an async worker. The daemon checkpoints WAL during graceful shutdown.

## IPC and Termux

IPC refuses non-loopback binds. Bearer credentials are split by privilege and compared in constant time: the limited client token reaches command/status/log routes, while a separate root-only admin token is required for snapshot/config/token-test/client-provisioning routes. Both are generated on first daemon start and stored as secrets. The module watchdog writes a root-owned client config from only the limited token; the managed Termux wrapper never prints or copies it into the Termux home.

Normal Termux commands post to `/v1/command`, so they use the same semantic core as Telegram. `xiao logs` calls the redacted log endpoint. The flashable module installs a managed Termux wrapper that elevates only the fixed module CLI, points it at the root-owned limited client config, and quotes every forwarded argument. `admin` subcommands require the separate local admin credential; they are for the root-owned module/WebUI context, not model-generated commands.

## KernelSU lifecycle

`post-fs-data.sh` initializes private directories and removes reboot-stale runtime PID files. `service.sh` runs during late-start, waits only a bounded time for Android boot completion, synchronizes the Termux wrappers, and detaches `watchdog.sh`. The watchdog tracks the child PID by executable identity, forwards termination, provisions the limited local client config, applies bounded exponential restart backoff, honors `auto_restart`, and bounds log growth. Mutable files live in `/data/adb/xiao`; module updates replace only `/data/adb/modules/xiao` content.

WebUI restart never tears down and respawns the supervisor from the short-lived
KernelSU WebUI execution context. It writes an explicit restart request, sends
TERM only to the owned `xiaod` child, and waits until the persistent watchdog
publishes a replacement PID. Explicit restart bypasses crash backoff, while a
full stop still shuts down both processes.

The module WebUI is intentionally narrow: one Gateway/Daemon status rail,
Restart/Refresh, a Telegram form containing only bot token and one Chat ID, and
an OpenAI-compatible Custom provider form. Codex and Antigravity OAuth never
appear as WebUI credential fields; `/login` in Telegram owns those flows. The
Custom form discovers `/models` through authenticated root admin IPC and can
reuse the stored API key without returning it to JavaScript. All WebUI admin
commands pass through `action.sh`, which supplies the same explicit root paths
as the watchdog. Telegram ACL and Custom provider changes hot reload; a new bot
token requests a daemon restart after validation.

## Security invariants

There is no generic shell tool callable by the model. Root shell execution
exists only in fixed KernelSU lifecycle/WebUI administration scripts whose
arguments are encoded and whose binaries are fixed paths. Telegram
authorization happens before agent execution. The IPC socket is loopback-only
and bearer-authenticated. Snapshot APIs return only whether a token/key is
configured, never its value. Surfaced log/error text passes through redaction.
