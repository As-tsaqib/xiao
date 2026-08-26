# 12 — Telegram Runtime UX

## Preserve current simplified public commands

v0.3.1 is runtime optimization, not another Telegram command expansion. Preserve the current v0.3 public surface unless a real bug requires a compatibility fix.

Expected public daily-use set remains centered on:

```text
/start
/help
/login
/model
/new    /n
/sessions /s
/btw
/status
/context
/retry  /r
/yolo   /y
/stop
/skills
/tools
```

Do not reintroduce `/memory`, `/doctor`, `/approvals`, `/approve`, `/deny`, `/provider`, `/account`, or `/session` into Telegram help/menu.

## Streaming draft

During generation, one draft message may contain:

- full bounded progress timeline;
- current active semantic action icon;
- accumulated visible assistant answer text as it streams.

The final permanent response uses the existing rich rendering path and contains no progress block.

## Thinking label

Do not use a generic “Thinking…” row for time spent on unrelated internal background work.

Foreground timeline should identify safe observable stages such as:

```text
Analyzing request
Reading image
Running command
Checking result
Writing response
```

Post-delivery memory/skill learning is not shown as blocking Thinking.

## Error UX

Vision Unknown must no longer emit “selected model does not declare vision capability” before a real attempt. If an exact provider response proves image input unsupported, error should say what was actually observed and offer `/model` or WebUI model/capability management.

Turn exhaustion at 150 should be rare. If reached, return a bounded useful Blocked result with last observable progress rather than a bare internal-style error.

## Stop

`/stop` must cancel:

- active provider SSE stream;
- parallel tool group;
- active `termux_job` child processes;
- attachment processing in the same lineage;
- foreground semantic verification.

It does not need to cancel already post-delivery background learning for a completed earlier run.

## Android Composer Icon Limitation

Telegram Android client does not permit external bot control over composer action icons or composer styling via draft payload attributes. Xiao only renders draft progress and rich view blocks in the message stream; it does not claim or attempt composer icon control on Android.
