# 08 — Termux and Android Security

## Execution backends

```text
ordinary CLI / userland
        ↓
TermuxExecutor
(unprivileged Termux UID)

privileged Android operation
        ↓
AndroidBroker
(typed operation + policy)
```

No raw unrestricted root shell is a model tool.

## Hard DENY in Termux

- `su`
- `tsu`
- `sudo`
- `doas`
- `sh -c`
- `bash -c`
- opaque pipeline/redirect/semicolon command strings
- direct secret exfiltration
- path traversal or symlink escape from required workspace
- execution of a cached script whose hash/interpreter/provenance verification fails

## Routine ALLOW

Structured argv commands that:
- execute under Termux UID;
- remain within capability/workspace constraints;
- are not privileged;
- are not secret-sensitive;
- do not cross a destructive boundary.

Examples:
- `rg`, `cat`, `python script.py`, `cargo test`, `ffmpeg`, `git status`, compiler commands.

## Destructive Termux nuance

Do not blanket-approve every `rm` forever, but do not require owner approval for safe autonomous cleanup inside Xiao's own session workspace.

Policy:
- destructive target canonicalizes under current session workspace → ALLOW;
- target outside session workspace but writable by Termux → ASK;
- root/system/sensitive path → DENY or typed AndroidBroker as appropriate.

This requires argument/path-aware validation, not program-name-only policy.

## Script policy

- verified Xiao-generated cached script under controlled path → ALLOW after policy re-check;
- arbitrary script path → ASK;
- shell `-c` string → DENY.

## AndroidBroker

Typed operations declare:
- operation name;
- exact arguments;
- read/mutate class;
- risk;
- evidence;
- rollback expectations where possible.

YOLO:
- session-scoped;
- converts eligible ASK to ALLOW;
- never bypasses hard DENY;
- every YOLO auto-approval is audited.

## Goal

Autonomous ordinary Termux work should be smooth, while root/device mutation remains explicit and typed.
