# 20 — Real Rooted Android Acceptance

CI is necessary but not sufficient. These checks are required on the intended rooted Android + Termux deployment before v0.3.1 release qualification.

## Setup

Record without secrets:

- Xiao exact commit SHA;
- Android version/device architecture;
- Termux package/version environment;
- root broker available yes/no;
- Custom endpoint alias;
- exact selected model id;
- configured protocol;
- capability state before test;
- agent settings snapshot.

Do not record API keys/tokens.

## Test A — Multimodal Unknown → Supported

1. Set exact model vision override to Auto.
2. Clear/invalidate automatic vision evidence for that exact model.
3. Confirm WebUI shows vision Unknown.
4. Send a normal Telegram photo with a simple question about visible content.
5. Expected: Xiao attempts the image request instead of refusing locally.
6. Expected: if provider succeeds, Xiao responds about the actual image.
7. Re-open WebUI/model status.
8. Expected: exact model now records runtime-confirmed Supported.

## Test B — Explicit unsupported

Use a mock/local endpoint or known text-only exact model.

1. vision state Auto/Unknown;
2. send image;
3. provider returns explicit normalized unsupported-image input response;
4. Xiao produces truthful error;
5. exact model becomes Unsupported;
6. unrelated model remains unchanged.

## Test C — Streaming latency

1. Use a Custom endpoint known to support SSE.
2. Ask a normal no-tool question.
3. Observe Telegram draft.
4. Expected: visible answer text appears incrementally before full answer is complete.
5. Check WebUI Runs timing:
   - provider first byte;
   - first text delta;
   - final delivery.
6. Verify final permanent message is sent once.

## Test D — Tool streaming/continuation

Ask Xiao for a device inspection that requires Termux tools.

Expected lifecycle:

```text
stream/progress
→ tool call(s)
→ tool execution
→ continuation stream
→ final
```

No raw JSON/tool protocol appears in chat.

## Test E — Old 8-turn regression

Use a controlled task/fake local Custom endpoint or a real multi-stage task that legitimately exceeds 8 provider iterations.

Expected: no failure at 8; WebUI reports max turns 150. If task loops without progress, no-progress guard should still stop early rather than consume 150 blindly.

## Test F — Parallel read-only tools

Run a task that triggers several independent read-only observations. Confirm timings overlap and result ordering is stable.

## Test G — `termux_job`

Ask for an inspection that naturally needs multiple commands, such as RAM + processes + storage + uptime.

Expected: Xiao can choose one structured `termux_job`; all substeps execute under Termux UID; one aggregated result returns; audit shows each step.

## Test H — Root boundary

1. Try to make a Termux job invoke `su`/`tsu` or equivalent. Expected hard deny.
2. Trigger a typed root mutation with YOLO off. Expected exact smart approval.
3. Enable YOLO in current session and repeat a normally-ASK typed root action. Expected auto-grant with audit.
4. Hard-DENY operation remains denied in YOLO.

## Test I — `/stop`

Test `/stop` separately while:

- provider SSE is active;
- parallel read-only tools are active;
- `termux_job` child process is active.

Expected: cancellation propagates and no orphan process continues.

## Test J — Background learning does not block reply

Complete a verified reusable task.

Expected ordering from run timing:

```text
final Telegram delivery completed
<
learning started
```

or, if exact delivery ack is unavailable, prove the configured post-delivery delay/foreground priority prevents learning from extending user-visible final latency.

## Release record

Store a redacted validation document in the repository containing:

- exact SHA;
- CI run id;
- pass/fail per test A–J;
- measured timing summary;
- any device-specific waiver with reason.

Do not mark v0.3.1 ready if multimodal, streaming, cancellation, or root-boundary tests are unverified.
