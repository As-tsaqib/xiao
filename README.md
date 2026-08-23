# xiao v0.2.0

xiao is a private, single-owner, persistent Rust AI agent designed primarily
for a rooted Android device. Telegram is its primary interaction surface and
Termux is its ordinary general-purpose execution environment. `xiaod` owns
configuration, durable SQLite state, provider authentication, bounded context,
living identity/memory files, filesystem skills, and the bounded agent loop.
The `xiao` CLI and KernelSU WebUI remain administrative adapters over the same
semantic Command Core.

v0.2.0 implements persistent `SOUL.md`, `USER.md`, `MEMORY.md`, `AGENTS.md`,
and generated `ENVIRONMENT.md`; typed runtime probing and capability
resolution; provider-agnostic tools; controlled Termux execution with trusted
dependency installation; a typed Android privileged broker; SQLite FTS5
recall; verified completion; and trace-based learning into compatible
`skills/<name>/SKILL.md` files. Existing principal/session identifiers remain
as compatibility and isolation keys, not as multi-tenant product architecture.

The release ships as one root-level KernelSU/Magisk-compatible module ZIP.
Flashing that ZIP installs the daemon, watchdog, WebUI, and managed Termux
wrappers together. Mutable state stays outside the replaceable module payload.

## Install

1. Download `xiao-v0.2.0-kernelsu-arm64.zip`.
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
xiao session
xiao provider
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

- `xiaod` is the single source of truth.
- Telegram ACL is checked before business/provider work.
- SQLite enforces principal ownership for list, switch, rename, archive, and
  read operations.
- `/btw` uses an isolated principal-owned side session and never writes into
  its parent main session.
- Telegram intake is durable and dispatched asynchronously; slow generation
  does not block `/stop`, callbacks, or another principal.
- Inline menus are edit-first with stale-revision protection.
- Progress drafts are ephemeral and contain bounded safe status only; final
  answers are persistent Rich Message views, with verified result artifacts
  uploaded through Telegram `sendDocument`.
- Provider calls and tools use canonical typed interfaces. `ToolRegistry` and
  `ToolPolicy` expose bounded built-ins, the structured `termux_terminal`, and
  two typed Xiao-service Android operations. Shell command strings and a
  generic root shell are not exposed. Destructive/sensitive Termux calls and
  privileged service restart use exact, durable, one-shot approval.
- Missing ordinary binaries use a trusted Termux package mapping, validated
  package-manager argv, durable install audit, executable re-probe, and then
  resume the original command. There is no arbitrary remote installer path.
- Context is assembled from hard rules, SOUL, verified runtime/capabilities,
  USER, relevant MEMORY, AGENTS, selected skills, summaries, FTS excerpts,
  recent turns, and the current request under a character budget. Raw history
  is never deleted by compression.
- Action completion distinguishes verified success, not-yet-verified, blocked,
  and failed. A textual “done” is not evidence; the bounded loop continues for
  a changed strategy or an observable verification.
- Agent/tool/dependency boundaries are audited. Interrupted side effects are
  quarantined rather than blindly replayed.
- IPC is loopback-only with separate client/admin credentials.
- The managed Termux wrapper invokes KernelSU `su` only for the fixed,
  shell-quoted module binary; model output never reaches this root shell path.
- Secrets are outside normal config, private where supported, and redacted
  from surfaced logs/errors.
- `/compact` remains absent in v0.2.0; bounded summary creation is an internal
  ContextEngine responsibility.

Core semantic commands are `/new`, `/btw`, `/session`, `/model`, `/provider`,
`/account`, `/login`, `/logout`, `/status`, `/context`, `/stop`, `/retry`,
`/approvals`, `/approve`, `/deny`, `/settings`, `/help`, `/usage`, and
`/doctor`. Termux aliases preserve all arguments, so
`xiao session rename ID New Name` and
`xiao model MODEL_ID` reach those same variants.

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
- Custom: configure the base URL, protocol, models, and non-secret headers in
  config/admin input. API keys belong only in SecretStore through the dedicated
  admin field.

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
xiao-v0.2.0-kernelsu-arm64.zip
xiao-v0.2.0-kernelsu-arm64.zip.sha256
```

Run the workflow from a push/pull request or `workflow_dispatch`, then download
the `xiao-v0.2.0-kernelsu-arm64` artifact. Do not use an older local `dist/`
archive after changing source. A local source-only check is available and never
invokes Cargo:

```sh
./scripts/acceptance.sh --static-only
```

## KernelSU status

The Actions-built archive root contains `module.prop`,
installer/lifecycle/watchdog scripts, `skip_mount`, both arm64 binaries, the
example config, Termux wrapper template, and `webroot` assets. Installation
creates or preserves private mutable state under `/data/adb/xiao`; updates and
uninstall do not silently delete user sessions, accounts, or credentials. A
real flash/reboot and browser OAuth completion remain device integration checks.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/ACCEPTANCE.md](docs/ACCEPTANCE.md), and
[docs/REVISION_VALIDATION.md](docs/REVISION_VALIDATION.md).
