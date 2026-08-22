# Module artifact validation

Do not compile or package xiao locally. Trigger `.github/workflows/ci.yml` with
a push, pull request, or `workflow_dispatch`. The workflow is responsible for:

1. locked Rust fmt/check/Clippy/test/release gates;
2. an Android arm64 `cargo-ndk` build of `xiao` and `xiaod`;
3. two independent invocations of `packaging/build-module.sh`;
4. identical SHA-256 results for both ZIP builds;
5. sidecar verification and `unzip -t` integrity;
6. an artifact containing only the ZIP and `.zip.sha256` sidecar.

The archive must place `module.prop`, `customize.sh`, `post-fs-data.sh`,
`service.sh`, `watchdog.sh`, `action.sh`, `uninstall.sh`, `bin/`, `termux/`, and
`webroot/` directly at ZIP root. It must not contain a parent `xiao/` directory,
runtime databases, account secrets, logs, PID files, or client credentials.

After downloading and flashing the Actions artifact on arm64 Android, run
`xiao-ctl status`, `xiao status`, and `xiao doctor`. Then verify a real reboot,
wrapper repair, bounded logs, module update with preserved `/data/adb/xiao`,
OAuth callbacks, and uninstall restoration of pre-existing Termux commands.
