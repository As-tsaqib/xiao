# v0.1.0 acceptance coverage

This document maps the specification acceptance matrix to implementation and
automated/device validation. `scripts/acceptance.sh` is source-only and never
invokes Cargo. GitHub Actions is the sole authority for Rust formatting,
compilation, Clippy, tests, the Android arm64 build, and packaging the single
KernelSU module archive. Tests needing a real reboot, Telegram bot credential,
or provider account remain explicit device integration checks.

| ID | Coverage |
|---|---|
| A01 | GitHub Actions runs locked Rust fmt/check/test/Clippy/release gates; local acceptance is static-only. |
| A02 | CI `cargo-ndk` arm64 build plus one deterministic root-layout KernelSU module ZIP and SHA sidecar. |
| A03 | KernelSU `post-fs-data.sh` + `service.sh` + persistent `watchdog.sh`; device reboot check documented. |
| A04 | Module mutable paths use `/data/adb/xiao`; standalone quickstart uses private XDG config/data paths. Both remain outside replaceable binaries/module content. |
| A05 | Telegram adapter implements only `getUpdates` long polling in v0.1.0. |
| A06 | `telegram::acl` unit test and ACL-before-dispatch ordering. |
| A07 | `config::parse_id_list` unit test; the simplified WebUI submits one validated signed Chat ID. |
| A08 | Snapshot returns only `token_configured`; WebUI never receives the stored bot token. |
| A09 | WebUI/API token test calls Telegram `getMe` before config commit for a new token. |
| A10 | Session persistence/archive unit tests; `/new` calls `create_and_switch`. |
| A11 | `/session` is 5/page with table, numbered buttons, paging and management actions. |
| A12 | Session callback changes call edit-first on the existing menu session. |
| A13 | `telegram::menu::edit_first`. |
| A14 | Simulated edit-failure unit test verifies replacement and old keyboard retirement. |
| A15 | Callback revision/owner checks plus revision unit test. |
| A16 | Callback is ACKed before acquiring per-menu serialization lock. |
| A17 | `toggle_side` MAIN/SIDE unit test. |
| A18 | `agent_context` includes main then side context. |
| A19 | `side_never_writes_main` DB assertion. |
| A20 | `/btw` while SIDE exits to MAIN; no nested side creation. |
| A21 | Rich/plain final renderers add `SIDE CHAT SESSION`; unit test. |
| A22 | `/login` picker exposes Codex, AGY, Custom. |
| A23 | Auth broadcast watcher edits the existing menu on completion/failure. Real OAuth remains integration-tested with credentials. |
| A24 | `provider_accounts` schema + per-account credentials + provider account commands. |
| A25 | `/model` view/catalog/selection. |
| A26 | `/account` view/selection/logout/login management. |
| A27 | `/provider` selection and session rebinding. |
| A28 | Typed `Provider` trait/registry. |
| A29 | Progress uses `sendRichMessageDraft`, separate from final send; a 20-second heartbeat refreshes the same draft while active. |
| A30 | No CoT type exists; only typed safe `AgentEvent` statuses reach presentation. |
| A31 | Telegram progress labels are truncated/redacted/aggregated, never raw tool stdout. |
| A32 | `Progress` blocks are omitted for non-draft rendering; final-output unit test. |
| A33 | Progress updater ticks at ~750 ms with missed-tick skipping. |
| A34 | Adapter-level fake Telegram + fake slow-provider regression starts a long generation, processes principal B `/status` and a callback concurrently, then processes `/stop` and verifies cancellation happens before natural provider completion with no assistant row. |
| A35 | Retry unit test reuses latest user request without duplicate user row. |
| A36 | `/status` includes gateway, daemon, Telegram, provider/model/session/mode. |
| A37 | `/context` reports main/effective messages and isolation mode. |
| A38 | `/help` has edit-first Chat/AI/Accounts/Advanced categories plus direct topic help including `btw`, `session`, `model`, `account`, and `settings`. |
| A39 | Flashing the one module ZIP installs managed `xiao`/`xiao-ctl` wrappers; arguments are shell-quoted, only fixed module paths are elevated, and uninstall restores backed-up commands. |
| A40 | Termux `/v1/command` and Telegram both call `CommandCore`. |
| A41 | config validation rejects non-loopback IPC; unit test. |
| A42 | exact constant-time bearer check; separate limited client/admin tokens; unit test. |
| A43 | WebUI has only a compact Gateway/Daemon status rail plus lifecycle actions. |
| A44 | admin apply validates config and externally tests any new Telegram token before atomic save. |
| A45 | Chat ID ACL + Custom provider config update in memory without restart; event emitted. |
| A46 | SQLite reopen persistence unit test, WAL mode. |
| A47 | redaction unit test and redacted `/v1/logs`/error surface. |
| A48 | no provider/agent unrestricted shell tool; static acceptance grep. |
| A49 | MenuStore is in-memory TTL state independent of SQLite sessions. |
| A50 | README + architecture/protocol/operations/acceptance documentation. |
| A51 | First successful MAIN exchange gives default sessions a bounded automatic title. |
| A52 | Effective agent context is bounded while complete conversation history remains persisted; `/compact` is intentionally not exposed in v0.1.0. |
| A53 | `/usage` provides session message/character usage. |
| A54 | `/doctor` reports DB, IPC, Telegram transport, provider registration, and root-shell invariant. |
| A55 | Intentionally not enabled in v0.1.0: enrollment cannot bypass the “ACL before all Telegram work” invariant. Operators set the Chat ID explicitly in KernelSU WebUI. |

## Revision blocker regression gates

The v0.1.0 completion pass adds direct regression coverage for the release blockers found during review:

- **Principal isolation:** storage and session-manager tests prove principal A cannot list, switch, rename, archive, read history, or reach side sessions owned by principal B, including after reopening the SQLite database.
- **Responsive Telegram intake:** the adapter-level fake-server test proves a slow provider generation does not block another principal, callbacks, or `/stop`.
- **Rich final answers:** parser/renderer tests cover headings, paragraphs, fenced code with language, tables, bullet/ordered lists, blockquotes, inline emphasis/code/links, malformed markup preservation, and oversized answer pagination.
- **Atomic account activation:** tests cover fresh Custom→Codex, Custom→AGY, two Codex accounts, Codex→AGY, invalid/disconnected accounts, no-model failure, and complete rollback on failure.
- **Deployable OAuth:** static acceptance checks the CLIProxyAPI-compatible
  Codex PKCE parameters and Antigravity installed-app constants/scopes/project
  bootstrap. Neither login depends on a WebUI credential form.
- **Custom endpoint E2E:** the isolated device script starts xiaod with its
  boot-style environment, configures the already-installed CLIProxyAPI endpoint,
  activates the Custom provider, and requires a real `XIAO_E2E_OK` model reply.
- **Typed tool loop and streaming:** an agent test proves provider→typed tool→result→provider continuation; static acceptance rejects generic shell/root execution and checks incremental SSE consumption.
- **Durable update intake:** database tests cover accepted-but-unclaimed replay, duplicate acceptance idempotence, processing-crash quarantine, and failed-processing quarantine.
- **Termux alias parsing:** tests cover aliases with zero/one/multiple arguments, slash forms, explicit `chat`, and quoted natural-language prompts.

## Device integration checklist

After automated binary/archive acceptance, validate on an arm64 KernelSU Next device:

1. Install module, reboot, verify daemon PID/log/status and persistence under `/data/adb/xiao`.
2. Configure a real Telegram bot and Chat ID; verify updates from another chat cause no session/message/provider mutation.
3. Exercise `/session` paging/detail/rename/select/archive and intentionally force/edit an older menu to confirm stale callbacks do nothing.
4. Run a generation and inspect Telegram history: progress must be draft-only; final must contain no progress/CoT. Test `/stop` and `/retry`.
5. Enter/exit `/btw`; query something relying on MAIN context; confirm SQLite MAIN history contains none of the SIDE messages/reply.
6. Complete Codex and Antigravity OAuth with authorized test accounts and verify the existing login menu updates, token refresh, multi-account selection, and model calls.
7. Configure a disposable OpenAI-compatible custom endpoint and key; fetch its model catalog, verify invalid URL/model input is rejected, and confirm valid config hot-reloads.
8. Verify the module-created `xiao`/`xiao-ctl` Termux wrappers, then run status, commands/chat, logs, restart, and uninstall/backup-restore checks.
9. Restart daemon and reboot device; verify sessions, accounts, config, and Telegram offset remain durable.
