# First-patch validation record — 2026-08-22

> Historical v0.1.0 device evidence. Xiao v0.2.0 host validation and its
> still-pending device/provider checks are tracked in `docs/ACCEPTANCE.md`.

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
- WebUI restart keeps that boot-owned watchdog alive, requests replacement of
  only its `xiaod` child, and waits for the replacement process instead of
  launching a new supervisor from the short-lived WebUI execution context.
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

After installing the GitHub Actions artifact for commit `5bec12b`, the first
WebUI Telegram save validated and persisted both values, but its old full
supervisor restart left both processes stopped. The token independently passed
Telegram `getMe`; after lifecycle recovery, the live snapshot reported a valid
stored token, the requested Chat ID, Gateway `running`, and Telegram `polling`.
This observation is the regression case for the child-only restart handshake.

The same installed artifact received Telegram commands and completed Custom
provider generation, but Telegram rejected its final outbound message with
`400 Bad Request: object expected as reply markup`. Request inspection traced
that to `reply_markup: null`; optional Bot API fields are now omitted when
absent, with a regression test covering the exact JSON shape.

The already-installed CLIProxyAPI module exposed
`http://127.0.0.1:8317/v1/models` with 23 models. The isolated device E2E
procedure used a temporary `/data/adb/xiao-e2e.*` home, left production xiao
data/config untouched, and completed its boot and real-generation legs through
xiao's Command Core:

```text
PASS  xiaod boot-style environment started successfully
PASS  xiao discovered custom model gpt-5.6-luna through CLIProxyAPI /v1/models
PASS  CLIProxyAPI custom model gpt-5.6-luna returned XIAO_E2E_OK through xiao CommandCore
```

The temporary home was removed by the test cleanup trap.

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

Commit `5bec12b` passed all of those gates in GitHub Actions run `32577699987`;
its artifact was the installed binary used for the device evidence above. Each
later lifecycle correction still requires its own green run and a newly
flashed artifact.

Still deferred for the child-only restart correction:

- a real reboot and WebUI Telegram-save restart using its new Actions artifact;
- real Codex and Antigravity browser completion, refresh, and generation.
