# xiao v0.1.0

xiao is a Rust AI-agent gateway for Android. `xiaod` owns configuration,
SQLite state, principal-scoped sessions, provider authentication, provider
transports, and agent execution. The `xiao` CLI, Telegram, and KernelSU WebUI
are adapters over the same semantic Command Core.

The standalone Termux binary pair and KernelSU module are built from the same
validated Android arm64 release binaries. Mutable state stays outside both
replaceable distributions.

## Standalone Termux quickstart

Build both native Android arm64 binaries from this Termux checkout:

```sh
cargo build --locked --release --all-features
./target/release/xiao quickstart
```

`quickstart` is non-destructive and safe to rerun. It:

- creates a private standalone config and data layout;
- starts the sibling/installed `xiaod` as a detached process;
- waits for authenticated loopback IPC readiness;
- creates the role-limited client credential without printing it;
- preserves existing config, database, secrets, and client identity.

The standalone defaults are:

- daemon config: `$XDG_CONFIG_HOME/xiao/config.toml` or
  `$HOME/.config/xiao/config.toml`;
- client config: `$XDG_CONFIG_HOME/xiao/client.toml` or
  `$HOME/.config/xiao/client.toml`;
- mutable data: `$XDG_DATA_HOME/xiao` or `$HOME/.local/share/xiao`;
- IPC: authenticated HTTP on `127.0.0.1:37921` only.

Useful first commands:

```sh
xiao daemon status
xiao status
xiao doctor
xiao session
xiao provider
xiao model
xiao login codex
xiao chat "hello"
xiao daemon logs 100
xiao daemon restart
xiao daemon stop
```

Use `xiao quickstart --no-start` to prepare files without starting the daemon,
and `xiao daemon foreground` when you want the process attached to the current
terminal. `xiao config path` shows paths without revealing credentials;
`xiao config check` validates daemon/client config.

See [docs/TERMUX.md](docs/TERMUX.md) and
[docs/BINARY_TEST.md](docs/BINARY_TEST.md) for bundle and isolated-test usage.

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
  answers are persistent Rich Message views.
- Provider calls and tools use typed interfaces. There is no unrestricted
  model-generated string to root shell path.
- IPC is loopback-only with separate client/admin credentials.
- Normal Termux operation never invokes `su`.
- Secrets are outside normal config, private where supported, and redacted
  from surfaced logs/errors.
- `/compact` remains absent in v0.1.0.

Core semantic commands are `/new`, `/btw`, `/session`, `/model`, `/provider`,
`/account`, `/login`, `/logout`, `/status`, `/context`, `/stop`, `/retry`,
`/settings`, `/help`, `/usage`, and `/doctor`. Termux aliases preserve all
arguments, so `xiao session rename ID New Name` and
`xiao model MODEL_ID` reach those same variants.

## Provider setup

- Codex: `xiao login codex` starts device authorization.
- Antigravity: configure the deployment's own Google Desktop OAuth client ID
  in `config.toml` or via the admin/WebUI path, then run
  `xiao login antigravity`. No third-party private OAuth credential is bundled.
- Custom: configure the base URL, protocol, models, and non-secret headers in
  config/admin input. API keys belong only in SecretStore through the dedicated
  admin field.

Termux administrators can apply a JSON request without putting secrets in the
process list:

```sh
xiao admin apply-file /private/path/settings.json
xiao admin test-token-file /private/path/telegram-token.txt
```

Real OAuth, provider generation, and Telegram delivery require the user's own
credentials. The automated gate deliberately does not impersonate those
checks.

## Build and validation

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets --all-features
cargo test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --locked --release --all-features
./scripts/acceptance.sh --require-cargo
```

Android cross-build from a non-Android host:

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk --locked
cargo ndk -t arm64-v8a build --locked --release --bin xiaod --bin xiao
./packaging/build-termux.sh
./packaging/build-module.sh
```

On Termux itself, the normal release build is already an Android arm64 build.
Create the standalone bundle with:

```sh
XIAO_TARGET=native ./packaging/build-termux.sh
XIAO_TARGET=native ./packaging/build-module.sh
```

The Termux bundle contains `xiao`, `xiaod`, an installer, documentation, and
internal binary SHA-256 checksums. Both packaging scripts also create a
`.zip.sha256` sidecar in `dist/`.

## KernelSU status

The current module archive is `dist/xiao-v0.1.0-kernelsu-arm64.zip`. Its root
contains `module.prop`, installer/lifecycle scripts, `skip_mount`, both arm64
binaries, the example config, and synchronized `webroot` assets. Installation
creates or preserves private mutable state under `/data/adb/xiao`; updates and
uninstall do not silently delete user sessions, accounts, or credentials.

Packaging and archive validation pass locally on Android arm64. A real
KernelSU Next install/reboot/WebUI smoke remains a device integration check;
see the acceptance checklist before treating that hardware path as verified.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md),
[docs/ACCEPTANCE.md](docs/ACCEPTANCE.md), and
[docs/REVISION_VALIDATION.md](docs/REVISION_VALIDATION.md).
