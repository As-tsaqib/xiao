# 11 — Termux Workshop and Root Broker Policy

## Goal

Make Xiao useful without needless approval friction while maintaining a hard root boundary.

## Termux workshop

General CLI tools execute under the Termux UID and session workspace. v0.3.1 policy should treat that environment as the normal autonomous workshop.

### Default ALLOW

Structured Termux argv commands do not require approval solely because they modify files/processes accessible to the Termux UID.

Examples include normal use of:

- git;
- cargo;
- python;
- ffmpeg;
- package managers through DependencyResolver;
- rm/mv/cp inside Termux permissions;
- build/test scripts stored as audited files.

The runtime still logs risk/effect and preserves cancellation.

## Hard privilege boundary

The Termux executor must block attempts to obtain root or jump into Android privileged execution, including known privilege escalation binaries/paths. Root work must go through typed AndroidBroker tools.

Do not allow a cached script or `termux_job` to bypass this.

## Shell strings

Keep the structured-argv principle:

- model-supplied `bash -c`, `sh -c`, or equivalent opaque command strings remain hard-denied;
- file-backed scripts may execute if their file path, interpreter, content hash/source, and arguments are auditable;
- a generated helper script must live in the session workspace/cache and pass cache policy.

This lets script caching work without reopening an arbitrary root/shell injection surface.

## Root smart approval

Typed privileged AndroidBroker calls:

- ReadOnly safe status → may ALLOW;
- privileged mutating action → ASK when YOLO off;
- same ASK → auto-grant with audit when session YOLO on;
- Hard DENY remains DENY regardless of YOLO.

Telegram does not need `/approve` or `/deny` slash commands. If approval is required, the active run may render contextual inline Approve/Deny actions associated with the exact request id.

## Security acceptance

- `termux_job` cannot execute `su`/`tsu`/equivalent;
- direct Termux root escalation is denied even in YOLO;
- typed AndroidBroker ASK is auto-approved only in that exact YOLO session;
- no reusable broad grant is created from an approval.
