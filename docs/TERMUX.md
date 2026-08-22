# xiao managed Termux wrapper

There is no separate Termux package. Flashing the xiao module installs two
managed commands directly into the existing Termux prefix:

- `xiao` runs the module CLI against the module-owned daemon;
- `xiao-ctl` controls and diagnoses the watchdog lifecycle.

The wrapper is installed during module customization, synchronized again at
boot, and repaired from the module Action screen. If an unrelated command with
the same name already exists, it is moved to a module-owned backup first. The
backup is restored when xiao is uninstalled.

Examples:

```sh
xiao status
xiao doctor
xiao session
xiao login codex
xiao login antigravity
xiao chat "Explain the current session"
xiao daemon status
xiao daemon restart
xiao daemon logs 100
xiao-ctl status
```

The wrapper locates KernelSU's `su`, shell-quotes every forwarded argument, and
executes only fixed files below `/data/adb/modules/xiao`. It supplies:

```text
XIAO_CONFIG=/data/adb/xiao/config.toml
XIAO_CLIENT_CONFIG=/data/adb/xiao/client.toml
XIAO_HOME=/data/adb/xiao
```

`watchdog.sh` creates the private loopback client config after xiaod generates
its role-limited IPC credential. That file remains root-owned under
`/data/adb/xiao`; it is never copied into a normal Termux home or printed by the
wrapper.

Codex and Antigravity login URLs should be opened on the same Android device so
their browser redirects can reach xiao's temporary localhost callback listener.
