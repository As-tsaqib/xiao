# 09 — Memory, Skills, Background Learning

## Foreground rule

Do not perform memory evaluation, skill synthesis, or skill dedup before final response delivery.

## Pipeline

```text
VerifiedSuccess
   ↓
persist learning job (pending delivery)
   ↓
AgentAnswer ready
   ↓
frontend sends final successfully
   ↓
delivery ACK
   ↓
job becomes claimable
   ↓
background worker
   ├─ memory evaluator
   ├─ skill candidate synthesis
   ├─ semantic/deterministic dedup
   └─ canonical update/create
```

## Memory

Canonical current state:
- `USER.md`
- `MEMORY.md`
- SQLite indexing/history.

Operations:
- CREATE
- UPDATE
- MERGE
- DELETE
- NONE

Do not append duplicate preference entries when a current key can be updated.

## Skills

Only verified reusable workflows become learned skills.

Canonical format:
- AgentSkills-compatible `SKILL.md`;
- optional scripts/references/templates/assets;
- progressive disclosure.

Failed attempts may become Pitfalls only after eventual verified success.

## Delivery acknowledgement

This is cross-frontend.

Telegram:
- ACK after final permanent message and required artifact delivery.

CLI:
- ACK only after final answer successfully writes to stdout/output target.

WebUI/chat frontend, if present:
- ACK only after final response is delivered to the frontend transport.

If delivery fails:
- learning remains pending;
- do not learn from an answer the owner never received.

## Recovery

Learning jobs survive daemon restart.
Stale leases are reclaimable.
Exactly-once is preferred; at-least-once processing must be idempotent.
