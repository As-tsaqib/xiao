# First-patch validation record — 2026-08-22

This record separates checks that were actually executed on the rooted Android
device from checks deliberately deferred to GitHub Actions. Any ZIP currently
present in local `dist/` predates the final source changes and is not a release
candidate.

## Implemented source changes

- One flashable module root owns both binaries, WebUI, lifecycle scripts,
  watchdog, persistent-data initialization, and managed Termux wrappers.
- `post-fs-data.sh` clears reboot-stale ownership files; `service.sh` starts a
  detached watchdog; `watchdog.sh` validates PID ownership, bounds logs, applies
  restart backoff, and supplies `HOME`, `XIAO_HOME`, `XIAO_CONFIG`,
  `XIAO_CLIENT_CONFIG`, and `TMPDIR` explicitly to xiaod.
- The WebUI exposes only Gateway/Daemon status, lifecycle actions, two Telegram
  fields (Bot token and Chat ID), and an OpenAI-compatible Custom provider.
  Codex/Antigravity login remains in Telegram `/login` commands.
- Custom model discovery queries `{base_url}/models` through authenticated
  admin IPC, supports the OpenAI model-list shape, and can reuse a stored API
  key without returning the key to the browser.
- Codex OAuth follows CLIProxyAPI's browser Authorization Code + PKCE contract.
  Antigravity follows its installed-app client/scopes, localhost callback,
  userinfo, `loadCodeAssist`, and `onboardUser` contract.

## Executed device evidence

The installed module originally failed during boot-style launch with:

```text
Error: HOME is not set; set HOME or the explicit XIAO_* paths
```

After applying the boot-environment shell patch, the installed lifecycle
reported both processes active and autostart enabled:

```json
{"daemon":{"running":true,"pid":16569},"watchdog":{"running":true,"pid":16557},"autostart":true}
```

PIDs are observational and may change after restart. IPC was reachable at
`127.0.0.1:37921`.

The already-installed CLIProxyAPI module exposed
`http://127.0.0.1:8317/v1/models` with 23 models. The isolated device E2E
procedure used a temporary `/data/adb/xiao-e2e.*` home, left production xiao
data/config untouched, and completed its boot and real-generation legs through
xiao's Command Core:

```text
PASS  xiaod boot-style environment started successfully
PASS  CLIProxyAPI custom model gpt-5.6-luna returned XIAO_E2E_OK through xiao CommandCore
```

The temporary home was removed by the test cleanup trap.

The checked-in `scripts/device-custom-e2e.sh` now also requires xiao's new
authenticated model-discovery endpoint to return the selected model before it
applies config. That added leg needs the new GitHub-Actions-built binary and is
therefore not claimed as executed against the older installed binary.

## Local validation boundary

Local checks for this patch are intentionally limited to source/static tools:

```text
sh -n / bash -n
shellcheck
node --check
TOML parsing
scripts/acceptance.sh --static-only
git diff --check
```

No local Cargo build, Rust test, Clippy run, Android cross-compile, module ZIP,
or replacement checksum is valid evidence for the final patch.

## GitHub Actions authority

The `ci` workflow must pass before release. It runs Rust fmt/check/Clippy/tests
and a host release build, then cross-compiles Android arm64 with `cargo-ndk`.
The Android job packages twice, compares ZIP hashes, verifies the SHA sidecar
and ZIP integrity, and uploads exactly the module ZIP plus sidecar.

Still deferred until that workflow and a newly flashed artifact are available:

- Rust compile/test/Clippy/format results for the changed source;
- deterministic final ZIP name/hash and archive contents;
- a real reboot after flashing the new Actions artifact;
- KernelSU WebUI rendering/bridge behavior from that artifact;
- real Codex and Antigravity browser completion, refresh, and generation.
