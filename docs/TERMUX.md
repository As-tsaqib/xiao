# xiao standalone Termux bundle v0.1.0

This bundle contains both Android arm64 binaries:

- `xiao`: non-root CLI and standalone lifecycle manager;
- `xiaod`: the daemon and single source of truth.

Run them directly from the extracted directory:

```sh
chmod 755 xiao xiaod install-client.sh
./xiao quickstart
./xiao daemon status
./xiao status
./xiao doctor
```

Because `xiaod` is beside `xiao`, daemon discovery is automatic. The generated
client credential is private and is not printed by quickstart.

To install both binaries into the current Termux prefix:

```sh
./install-client.sh standalone ./xiao ./xiaod
xiao quickstart
```

Lifecycle commands are idempotent and scoped to the selected xiao config:

```sh
xiao daemon start
xiao daemon foreground
xiao daemon status
xiao daemon logs 100
xiao daemon restart
xiao daemon stop
```

Agent/session examples:

```sh
xiao status
xiao session
xiao new
xiao btw
xiao context
xiao provider
xiao model
xiao login codex
xiao chat "Explain the current session"
xiao logs 100
```

Provider generation requires an authorized account or configured custom API.
Until then, chat/retry returns a clear error such as `account not selected`;
daemon/session/status/doctor/setup functions remain usable.

For a CLI paired to the KernelSU daemon instead of a standalone daemon, use a
private pairing TOML file:

```sh
./install-client.sh pair ./xiao /private/path/pairing.toml
```

Normal CLI operation talks only to authenticated loopback HTTP and never uses
`su`. The client rejects non-loopback endpoints in v0.1.0.
