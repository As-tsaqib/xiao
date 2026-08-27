# 20 — Real Rooted Android Acceptance

Record:
- device model;
- Android version;
- KernelSU/KernelSU-Next version;
- Termux package/version;
- Xiao exact SHA;
- provider endpoint;
- exact model;
- network;
- timestamps/latency.

## A — Unknown → Supported vision
Use an exact Custom model whose vision state starts Unknown. Send Telegram Android photo "Apa ini". Verify:
- private attachment row ready;
- provider received image;
- useful answer;
- run not Blocked;
- capability becomes Supported.

## B — Explicit unsupported isolation
One exact model/profile returns explicit image unsupported. Verify only that scoped evidence becomes Unsupported; another profile/model stays unchanged.

## C — Telegram visible SSE
Verify first visible text appears in draft before provider completion and only one permanent final exists.

## D — Tool continuation SSE
Provider streams tool-call deltas. Verify assembled call is exact and no raw JSON/tool args leak into user text.

## E — Long controlled task
Task exceeds eight provider turns and completes or blocks only on real budget/progress rules.

## F — Mixed read-only scheduling
Use a batch with read/read/write/read/read. Verify first reads overlap, write is a barrier, later reads overlap, result order stable.

## G — `termux_job`
Run multiple safe steps under Termux UID. Verify substep audit and no root UID.

## H — Root boundary
Attempt `su`, `sudo`, shell `-c`; verify hard deny. Exercise a typed privileged broker op requiring approval. Verify YOLO semantics.

## I — `/stop`
Cancel during:
- provider SSE;
- parallel reads;
- `termux_job`;
- OCR/render if practical.
Verify durable interrupted states.

## J — Delivery before learning
Verify `final_frontend_delivery` precedes `background_learning_started`.

## K — Chat leave/re-enter
Start long streaming response, leave chat, reopen. Verify same draft resumes/replays latest snapshot and no duplicate final.

## L — Direct-final modes
Test `direct_final=true` and `false`:
- true shows visible answer text in ephemeral draft;
- false shows progress-only draft;
- both clear draft and send one permanent final.

## M — PDF long/multilingual
Create a long multi-page PDF and a multilingual sample. Verify parseability, no silent text corruption, and Telegram artifact delivery.

## N — Repair same command
Cause a command to fail, alter state, rerun the same command, and succeed. Verify no false ping-pong/no-progress block.

## O — WebUI on Android
Verify:
- safe-area header;
- no remote network required for UI CSS;
- System/Light/Dark theme;
- Android back through subpages;
- reduced-motion setting;
- session AI dialog works;
- profile edit works;
- capability override works;
- run timing visible.

## P — Composer note
Observe Android composer three-dot behavior for documentation only. Do not fail release solely on client-owned UI.
