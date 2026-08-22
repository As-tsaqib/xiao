# xiao v0.1.0 revision validation

This document records executed evidence for the current continuation pass on
2026-08-22 in native Termux Android (`aarch64-linux-android`). The original
architecture specification archive was not available; the revised source and
its architecture/acceptance documents were used as the v0.1.0 scope reference.

The public project/binaries were renamed from the pre-release name to `xiao`
and `xiaod`. This is still the existing daemon/core architecture, not a rewrite.
The standalone path and KernelSU archive are both current in this pass. Actual
KernelSU installation/reboot remains an explicit device integration check.

## Toolchain and lockfile

```text
cargo 1.97.1
rustc 1.97.1
host: aarch64-linux-android
```

`cargo generate-lockfile` completed successfully after the package rename. The
checked-in lockfile is Cargo-generated and currently resolves 220 packages.

## Rust and repository gates

The following commands were executed from the repository root after the
standalone implementation and passed:

```text
cargo fmt --all -- --check                                      PASS
cargo check --locked --all-targets --all-features               PASS
cargo test --locked --all-targets --all-features                PASS
  library tests                                                  57 passed
  CLI tests                                                       3 passed
  semantic integration tests                                     3 passed
  total                                                          63 passed
cargo clippy --locked --all-targets --all-features -- -D warnings
                                                                 PASS
cargo build --locked --release --all-features                   PASS
./scripts/acceptance.sh --require-cargo                          PASS
```

The original P0/P1 tests remain intact: principal ownership, side-session
ownership/reopen, asynchronous Telegram stop/callback/second-principal behavior,
cancellation timing, durable inbox replay/quarantine, Rich block parsing and
lossless pagination, atomic account transitions/rollback, typed tool
continuation, and CLI aliases with arguments.

New standalone tests cover:

- XDG/explicit path resolution;
- idempotent quickstart without config overwrite;
- private config/client/secret permissions;
- preserving an existing client identity;
- stale or executable-mismatched PID state removal without signaling;
- deterministic `xiaod` discovery from override, sibling, then `PATH`.

Shell syntax, WebUI JavaScript syntax, and example TOML parsing also pass as
part of the acceptance script.

## Native Android arm64 binary validation

The native Termux release build produced:

```text
target/release/xiao
target/release/xiaod
```

Both identify as ELF64 AArch64 Android API 24 binaries built with the
NDK-r29-compatible toolchain, use `/system/bin/linker64`, and depend only on
Android system `libdl.so`, `libm.so`, and `libc.so`. Networking remains
rustls-based; no OpenSSL runtime dependency was introduced.

Final binary hashes for this pass:

```text
82e43810ad576264429a73fc0e775cab881f32f1436618caad1408247c19999e  xiao
906df0b77a31284c80e10b3f5622a0f9ffa0c9debc30b95965d4cf25d11f01b6  xiaod
```

## Live standalone smoke

An isolated XDG config/data home was used. No developer database, config, or
credential was reused. The following surfaces passed using the release
binaries:

- `xiao --version`, `xiaod --version`, and local help;
- `xiao quickstart --no-start`, initial quickstart, and idempotent rerun;
- detached daemon survival after the invoking shell/PTY ended;
- `daemon start/status/logs/restart/stop` and authenticated readiness;
- config/client mode `0600`, secrets directory mode `0700`;
- quickstart output scan proving the client token was not printed;
- `config path`, `config check`, semantic `status`, and `doctor`;
- help/usage/settings/provider/model/account/session managers;
- new session, session rename with spaces, detail, `/btw` SIDE→MAIN, context,
  and idle `/stop`;
- provider/model alias arguments (`codex`, `gpt-5.6-sol`);
- admin snapshot and both IPC/local redacted log surfaces;
- graceful error bodies for missing account, failed chat/retry, and invalid
  account selection;
- graceful restart after the on-disk `xiaod` binary was replaced, followed by
  persistence verification of session name/provider/model.

The first smoke iteration exposed a real background-detachment defect: xiaod
was ready but exited when its invoking PTY ended. The lifecycle manager now
creates a dedicated Unix session with `setsid`; the repeated cross-shell smoke
passed.

## Standalone artifact

`XIAO_TARGET=native ./packaging/build-termux.sh` passed its ELF architecture,
Android linker, runtime dependency, checksum, and ZIP integrity checks. The
archive was then extracted into a fresh directory and its own binaries passed
quickstart, cross-shell status, config check, semantic status/doctor/new/session,
restart, post-restart readiness, and graceful stop.

```text
776f83d750edf73b067446fac5a6a9885a641555210069153499e49011e41adf
  xiao-v0.1.0-termux-arm64.zip
```

The archive contains only `xiao`, `xiaod`, installer/docs, the non-secret
isolated test config, and `SHA256SUMS`; scans found no database, logs, PID,
secret, or developer-machine config.

## KernelSU packaging validation

The module lifecycle and ZIP layout were rechecked against the current
KernelSU module/WebUI guides and the user's `As-tsaqib/picoclaw-module`
reference at commit `d27369c883ec55feeee2a782619070f775b1c413`.
The module uses a root-level `module.prop`, `customize.sh`, non-blocking
`service.sh`, `skip_mount`, `webroot/index.html`, and `/system/bin/sh` scripts.
KernelSU retains control of WebUI permissions/SELinux context.

The following additional checks passed:

```text
shellcheck -x -s sh module/*.sh                              PASS
shellcheck -s bash packaging/*.sh scripts/acceptance.sh      PASS
sh -n module/*.sh termux/install-client.sh                   PASS
bash -n packaging/*.sh scripts/acceptance.sh                 PASS
node --check webui/assets/app.js                              PASS
node --check webui/assets/ksu-bridge.js                       PASS
XIAO_TARGET=native ./packaging/build-module.sh                PASS (twice)
XIAO_TARGET=native ./packaging/build-termux.sh                PASS (twice)
```

Both repeated builds were byte-deterministic. The module was extracted into a
fresh directory and verified for ZIP integrity, required entries, permissions,
WebUI byte synchronization, binary equality with `target/release`, executable
`--version`, Android AArch64 ELF/linker metadata, system-only dependencies, and
absence of runtime databases, config, secrets, logs, PIDs, or client pairing
material.

```text
dd425717439c0fdb9e74f4df9acec68051c85a1360f675a1885b406824f653b9
  xiao-v0.1.0-kernelsu-arm64.zip
776f83d750edf73b067446fac5a6a9885a641555210069153499e49011e41adf
  xiao-v0.1.0-termux-arm64.zip
```

CI now cross-builds with `cargo ndk --locked`, packages both distributions,
and uploads both ZIPs and their SHA-256 sidecars.

## Deliberately deferred credential/device integration

Automated validation does not claim success for operations requiring user
authority or hardware state:

- real Codex device authorization, refresh, and generation;
- real Antigravity OAuth/PKCE, project discovery/onboarding, refresh, and model
  calls with the user's own OAuth client;
- a real custom provider/API key;
- real Telegram bot polling, ACL behavior, callbacks, rich drafts/finals;
- KernelSU module installation, reboot/supervisor behavior, WebUI bridge, and
  module-to-Termux pairing.

The KernelSU archive is current and locally validated. A real KernelSU Next
installation, reboot, supervisor, Action button, WebUI bridge, and pairing
smoke is deliberately not claimed without executing it through the manager on
a rooted device.
