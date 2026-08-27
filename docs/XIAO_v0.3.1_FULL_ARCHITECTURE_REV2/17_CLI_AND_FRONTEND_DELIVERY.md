# 17 — CLI and Frontend Delivery

## CLI

Keep:
- explicit `xiao chat` / `xiao ask`;
- strict command parsing;
- unknown command never becomes chat;
- human-readable output by default;
- stable `--json` envelope;
- no Telegram View schema;
- secrets never in argv when a safer file/stdin method exists.

## Delivery ACK

Background learning depends on actual delivery.

### Telegram
After:
- permanent final successfully sent;
- required artifacts successfully sent or recorded with explicit partial-delivery state.

Then:
- record `final_frontend_delivery`;
- release learning job.

### CLI
Daemon/CLI contract must retain internal `run_id`.
After stdout/file output succeeds:
- CLI calls local delivery ACK endpoint/action with run ID;
- xiaod records final delivery and releases job.

If CLI output fails (broken pipe, filesystem error):
- no ACK;
- no background positive learning yet.

### Other frontends
Use the same delivery service.

## Shared service

Prefer:

```text
FrontendDeliveryService::ack(run_id, frontend, metadata)
```

over separate ad hoc Telegram-only storage calls.

It must be idempotent.
