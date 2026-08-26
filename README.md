# xiao v0.3.0

xiao is a private, single-owner, persistent Rust AI agent designed primarily
for a rooted Android device. Telegram is its primary interaction surface and
Termux is its ordinary general-purpose execution environment. `xiao daemon` owns
configuration, durable SQLite state, provider authentication, bounded context,
living identity/memory files, filesystem skills, and the bounded agent loop.
The `xiao` CLI and KernelSU WebUI remain administrative adapters over the same
semantic Command Core.

v0.3.0 preserves those foundations while separating stable owner identity from
Telegram chat/topic scope, binding every approval to one exact run/tool call,
supporting isolated Custom provider profiles, and retaining bounded multi-turn
structured-tool continuation. Telegram now ingests validated images and
documents into private session storage with FTS5 retrieval, and the local Xiao
Manager exposes typed management views through authenticated `xiao` admin
actions. Existing principal identifiers remain compatibility keys during
migration, never a multi-tenant product boundary.

The release ships as one root-level KernelSU/Magisk-compatible module ZIP.
Flashing that ZIP installs the daemon, watchdog, WebUI, and managed Termux
wrappers together. Mutable state stays outside the replaceable module payload.

## Install

1. Download `xiao-v0.3.0-kernelsu-arm64.zip`.
2. Flash the ZIP in KernelSU Next (or a compatible module manager).
3. Reboot Android.
4. Open Termux and run:

```sh
xiao status
xiao doctor
xiao login codex
```

The archive has `module.prop`, lifecycle scripts, binaries, configuration, and
WebUI directly at its ZIP root—there is no nested folder and no second Termux
archive to install. During flash and every boot, the module safely installs or
updates `/data/data/com.termux/files/usr/bin/xiao` and `xiao-ctl`. An existing
unmanaged command is backed up and restored on module uninstall.

Runtime ownership is intentionally split:

- replaceable module: `/data/adb/modules/xiao`;
- persistent config/data/credentials/logs: `/data/adb/xiao`;
- daemon supervision: `post-fs-data.sh` + `service.sh` + `watchdog.sh`;
- Termux entrypoints: `xiao` and `xiao-ctl` managed wrappers.

Useful commands:

```text
xiao daemon status
xiao daemon restart
xiao daemon logs 100
xiao-ctl status
xiao status
xiao doctor
xiao sessions
xiao model
xiao login codex
xiao login antigravity
xiao chat "hello"
```

The wrapper elevates only the fixed module binary through KernelSU and supplies
the root-owned loopback client configuration. OAuth browser callbacks still
return to localhost on the same Android device.

## Architecture invariants

```text
Telegram Bot API (long polling) ─┐
                                ├─> semantic Command Core ─> Session/Agent Engine ─> Providers
Termux CLI -> auth loopback IPC ─┤                              │                    ├─ Codex
KernelSU WebUI -> admin IPC ─────┘                              │                    ├─ Antigravity
                                                               └─ SQLite + secrets └─ Custom
```

- `xiao daemon` is the single source of truth.
- Telegram ACL is checked before business/provider work.
- SQLite enforces principal ownership for list, switch, rename, archive, and
  read operations.
- Telegram conversations are scoped by chat plus `message_thread_id`; topic
  sessions, side chat, menus, callbacks, replies, and per-session YOLO state do
  not leak across topics. `/btw` never writes into its parent main session and
  starts with YOLO off.
- Telegram intake is durable and dispatched asynchronously; slow generation
  or semantic evaluation does not block `/cancel`, callbacks, or another owner
  scope.
- Inline menus are edit-first with stale-revision protection.
- Progress drafts are ephemeral and contain bounded safe status only; final
  answers are persistent Rich Message views, with verified result artifacts
  uploaded through Telegram `sendDocument`.
- Provider calls and tools use canonical typed interfaces. `ToolRegistry` and
  `ToolPolicy` expose bounded built-ins, the structured `termux_terminal`, and
  two typed Xiao-service Android operations. Shell command strings and a
  generic root shell are not exposed. Destructive/sensitive Termux calls and
  privileged service restart use exact, durable, one-shot approval.
- `ToolProtocol` explicitly distinguishes native continuation, strict
  structured JSON fallback, and `ChatOnly`; the runtime never silently removes
  tools and presents an action model as an equivalent agent.
- Missing ordinary binaries use a trusted Termux package mapping, validated
  package-manager argv, durable install audit, executable re-probe, and then
  resume the original command. There is no arbitrary remote installer path.
- Context is assembled from hard rules, SOUL, verified runtime/capabilities,
  USER, relevant MEMORY, AGENTS, selected skills, summaries, FTS excerpts,
  recent turns, and the current request under a character budget. Raw history
  is never deleted by compression.
- Action completion distinguishes verified success, not-yet-verified, blocked,
  and failed. A textual “done” is not evidence; the bounded loop continues for
  a changed strategy or observable verification. Each retry receives bounded
  runtime-owned `RUN_OBSERVATIONS`, without private reasoning.
- Semantic decisions for intent, completion interpretation, memory, reusable
  learning, skill synthesis, and equivalence are strict bounded JSON with one
  repair attempt and conservative fallback. They never receive tools or
  override deterministic security/evidence rules.
- Agent/tool/dependency boundaries are audited. Interrupted side effects are
  quarantined rather than blindly replayed.
- IPC is loopback-only with separate client/admin credentials.
- The managed Termux wrapper invokes KernelSU `su` only for the fixed,
  shell-quoted module binary; model output never reaches this root shell path.
- Secrets are outside normal config, private where supported, and redacted
  from surfaced logs/errors.
- `/compact` remains absent in v0.2.7; bounded summary creation is an internal
  ContextEngine responsibility.

The public Telegram command registry is exactly `/start`, `/help`, `/login`,
`/model`, `/new`, `/sessions`, `/btw`, `/status`, `/context`, `/cancel`,
`/retry`, `/yolo`, `/memory`, `/skills`, `/tools`, `/doctor`, and `/approvals`.
`/session` and `/stop` remain hidden compatibility aliases; account selection
and exact `/approve`/`/deny` actions remain internal menu syntax. `/provider`,
`/settings`, `/usage`, `/env`, `/about`, and `/logout` are intentionally absent.
Termux aliases preserve all arguments, so
`xiao sessions rename ID New Name` and `xiao model MODEL_ID` reach the same
Command Core variants.

The living workspace is under `[paths].data_dir` (normally
`/data/adb/xiao`). Owner edits to USER/MEMORY/SKILL files are reconciled into
SQLite indexes/history. `SOUL.md` is never rewritten by ordinary tasks;
`ENVIRONMENT.md` is refreshed from probes at startup.

## Provider setup

- Codex: `xiao login codex` uses CLIProxyAPI's browser Authorization Code +
  PKCE contract, including the `localhost:1455/auth/callback` redirect.
- Antigravity: `xiao login antigravity` works without extra OAuth setup using
  the CLIProxyAPI-compatible installed-app flow and
  `localhost:51121/oauth-callback`. It then resolves the account email and runs
  `loadCodeAssist`/`onboardUser`. An operator-owned Desktop OAuth client remains
  an advanced config/admin override, not a WebUI form.
- Custom: use `/login` → Custom in Telegram. The expiring wizard validates the
  endpoint, optionally captures and best-effort deletes the API-key message,
  stores credentials only in SecretStore, discovers/paginates models, probes
  the selected model's agent protocol, and requires confirmation. Callback
  state is bound to owner, chat, topic, menu, and expiry.

Termux administrators can apply a JSON request without putting secrets in the
process list:

```sh
xiao admin apply-file /private/path/settings.json
xiao admin test-token-file /private/path/telegram-token.txt
```

Real OAuth completion, provider generation, and Telegram delivery require the
user's own account/bot authorization. Automated tests validate the protocol
shape without impersonating those external checks.

## Build and validation

Release compilation and packaging are GitHub-Actions-only. The `ci` workflow
runs Rust formatting, check, Clippy, tests, a host release build, then the
Android arm64 `cargo-ndk` build. Its Android job builds the module twice,
requires byte-identical ZIP hashes, verifies the checksum and ZIP integrity,
and uploads exactly these two files:

```text
xiao-v0.3.0-kernelsu-arm64.zip
xiao-v0.3.0-kernelsu-arm64.zip.sha256
```

Authoritative green run for head `b9240c9`: **32701638246** (PR #1, run #153)
— `rust` PASS, `android-arm64` PASS, Rust 1.98.0, `cargo test` 312 passed / 0 failed,
deterministic `xiao-v0.3.0-kernelsu-arm64.zip` (11 415 927 B) with byte-identical
dual build and `sha256sum -c` + `unzip -t` verification. Full gate table and
P0/P1/P2 status: `docs/V031_VALIDATION.md`.

Exact gates (as executed in CI):

```sh
cargo fmt --all -- --check                          # PASS
cargo check --locked --all-targets --all-features   # PASS
cargo test --locked --all-targets --all-features    # PASS 312/0
cargo clippy --locked --all-targets --all-features -- -D warnings  # PASS
cargo build --locked --release --all-features       # PASS
shellcheck -x -s sh module/*.sh module/termux/xiao-wrapper scripts/device-custom-e2e.sh  # PASS
shellcheck -s bash packaging/build-module.sh scripts/acceptance.sh                        # PASS
node --check module/webroot/assets/app.js           # PASS
node --check module/webroot/assets/ksu-bridge.js    # PASS
./scripts/acceptance.sh --static-only               # PASS
cargo ndk -t arm64-v8a build --locked --release --bin xiao  # PASS
./packaging/build-module.sh ×2; sha256sum -c; unzip -t              # PASS
```

Run the workflow from a push/pull request or `workflow_dispatch`, then download
the `xiao-v0.3.0-kernelsu-arm64` artifact. Do not use an older local `dist/`
archive after changing source. A local source-only check is available and never
invokes Cargo:

```sh
./scripts/acceptance.sh --static-only
```

CLI contracts are frozen via `tests/snapshots/cli_help_body.txt` (validated in
`tests/cli_integration.rs`); see `docs/V031_VALIDATION.md` for the snapshot
guarantees.

## KernelSU status

The Actions-built archive root contains `module.prop`,
installer/lifecycle/watchdog scripts, `skip_mount`, the arm64 binary, the
example config, Termux wrapper template, and `webroot` assets. Installation
creates or preserves private mutable state under `/data/adb/xiao`; updates and
uninstall do not silently delete user sessions, accounts, or credentials. A
real flash/reboot and browser OAuth completion remain device integration checks.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/ACCEPTANCE.md](docs/ACCEPTANCE.md), and
[docs/REVISION_VALIDATION.md](docs/REVISION_VALIDATION.md).
