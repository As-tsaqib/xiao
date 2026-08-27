# 05 — Streaming and Telegram Drafts

## Canonical stream events

Provider adapters normalize into:

```text
Status
TextDelta
ToolCallDelta
Usage
Completed
```

The public AgentEvent layer may additionally contain tool lifecycle events and approvals.

Hidden reasoning is discarded.

## Custom SSE

Support exact implemented protocols:
- `openai_chat_completions`
- `openai_responses`

Do not show unsupported protocols in WebUI until a complete adapter exists.

Streaming fallback:
- if streaming fails before any visible/semantic partial data and failure explicitly indicates unsupported stream, retry once non-stream;
- after visible partial output, never retry in a way that duplicates user-visible text.

## Telegram draft model

A run has one stable draft identity.

```text
safe progress / TextDelta
       ↓
sendRichMessageDraft(same draft_id)
       ↓
repeat updates
       ↓
run completes
       ↓
clear draft exactly once
       ↓
send one permanent rich final
```

No permanent intermediate answer messages.

## `direct_final` compatibility semantics

Current compatibility behavior is retained:

- `direct_final = true`: visible `TextDelta` may appear in the ephemeral draft in addition to progress.
- `direct_final = false`: draft contains progress only; answer text appears only in the permanent final.

The naming is legacy and may be renamed after v0.3.1. Do not change semantics during this release without migration.

## Re-entry / replay

Leaving and reopening the Telegram chat during a run must not lose the current state.

Requirements:
- same draft ID;
- aggregator state is monotonic;
- periodic heartbeat re-sends the latest draft snapshot;
- re-entry must show the newest timeline/visible text on the next update;
- no new permanent message is created for replay.

## Timeline

Normal:
- 24 rows.

Detailed:
- 30 rows.

Minimal:
- 1 row.

Total progress text budget:
- approximately 3500 chars.

Only oldest terminal rows may be evicted; never evict the active row.

`GenerationCompleted` must finalize the active Writing row. It must **not** create a new synthetic `Finishing response` activity.

## Android emoji

Telegram Android may clip custom emoji in thinking/progress blocks.

Therefore:
- semantic `ProgressIcon` remains platform-independent;
- Telegram custom emoji is optional presentation;
- Android-safe Unicode fallback is always available;
- completed/failed rows remain static `✓` / `✗`.

The three-dot/composer UI is Telegram-client-owned and is not a Xiao server release gate.
