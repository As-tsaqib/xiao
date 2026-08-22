# Isolated Android arm64 binary test

The Termux bundle can be tested without installation and without touching a
normal xiao home. The supplied test config uses loopback port `38921` and keeps
all mutable state below the extracted directory.

```sh
unzip xiao-v0.1.0-termux-arm64.zip -d xiao-binary-test
cd xiao-binary-test
chmod 755 xiao xiaod
export XIAO_CONFIG="$PWD/config.termux-test.toml"
export XIAO_CLIENT_CONFIG="$PWD/xiao-test-data/client.toml"
./xiao quickstart
```

If the bundle does not include the optional test config, copy
`config/config.termux-test.toml` from the source tree first. Then exercise the
same public CLI that an installed build uses:

```sh
./xiao config check
./xiao daemon status
./xiao status
./xiao doctor
./xiao new
./xiao session
./xiao btw
./xiao context
./xiao daemon restart
./xiao daemon stop
```

`xiao quickstart` can be rerun before or after these commands; it must preserve
the config, client principal, database, and secrets. After `restart`, session
renames/provider/model selection must still be present.

Codex/Antigravity generation requires an authorized user account. Antigravity
also requires the deployment's own OAuth client ID. Telegram is disabled in
the isolated config so this smoke test cannot accidentally poll a real bot.

Test state remains in `./xiao-test-data`. Remove only that explicit directory
when you intentionally want to discard the isolated database and credentials.
