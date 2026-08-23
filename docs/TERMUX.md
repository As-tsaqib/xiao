# Xiao and Termux

## Owner CLI wrapper

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

## Agent execution backend

Termux is also Xiao's default general-purpose agent executor. At startup,
`EnvironmentProbe` discovers the real prefix, home, shell, package manager,
owner UID/GID, and selected binaries. The generated `ENVIRONMENT.md` records a
concise snapshot; current in-memory probes remain authoritative.

`termux_terminal` accepts one structured program plus argv. It does not accept
a shell command string. On a root module deployment, the child clears inherited
supplementary groups and drops to the detected Termux app UID/GID. Its PATH is
limited to directories below the detected Termux prefix, and its default cwd is
Termux home because the Xiao identity data root is intentionally root-private.
On Linux/Android it also sets `no_new_privs` before exec. The executor captures exit status/stdout/stderr and enforces timeout,
cancellation, output retention bounds, controlled environment keys, and secret
redaction.

Runtime policy rejects privilege-escalation/system commands, `sh -c`, direct
package mutations, and automatic installer pipelines. Clearly destructive
argv, an opaque shell script, or credential-sensitive paths require an exact
owner approval (`/approvals`, `/approve <id>`, `/deny <id>`). Approval is
argument-bound, expires, and is consumed once. This terminal is user-space
general execution, not an unrestricted root shell.

If a requested ordinary binary is missing, Xiao checks a small trusted
binary-to-Termux-package mapping. It invokes only the detected `pkg`/`apt`
backend with a normalized package name, records progress/audit state, re-probes
the executable, and resumes the original command. Unknown binaries and remote
installer scripts are not auto-installed.

Result files can be declared by a terminal call. Xiao accepts only bounded
regular files under the controlled task cwd/workspace, revalidates the path
against its data root or Termux home at the Telegram boundary, and uploads the
file with `sendDocument`.
