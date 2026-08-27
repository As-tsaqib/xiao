# 12 — Telegram UX Bugfix Matrix

This file turns reported bugs into release contracts.

## Already implemented but must remain protected by tests

### `/new` preserves AI binding
New session inherits provider/profile/model and resets YOLO off.

### `/login` alias suffix
Collision automatically chooses `custom_1`, `custom_2`, etc.

### `/model` one-tap
Selection must not wait for a probe.

### `/provider` vs `/model`
Different screens and responsibilities.

### Android progress emoji
Unicode fallback must render when custom emoji is clipped/unsupported.

### Photo vision
Telegram photo must not become false Blocked after a valid model answer.

### Private attachment persistence
Photo/document is stored privately and the provider receives the stored content.

### Direct-final / live draft lifecycle
Draft is ephemeral, cleared once, final is permanent once.

### Informational prompt tool filtering
Questions/code examples do not opportunistically run Termux.

### Result-aware repair flow
Repeating the same command after a changed state/result must not trigger false ping-pong detection.

### PDF
Deterministic, workspace-relative, parseable.

### Termux path security
No absolute cwd traversal or symlink escape.

### CLI
Strict syntax and human-readable output.

## Required device retest

1. Send a Telegram Android photo with "Apa ini".
2. Compare `direct_final=true` and `false`.
3. Leave the chat while streaming, reopen, verify draft replay/heartbeat.
4. Confirm only one permanent final.
5. Confirm custom emoji fallback is readable on Android.
6. Verify PDF long/multilingual delivery.
7. Trigger a repair flow that reuses the same command after file/state change.
8. Run all rooted Android acceptance gates.

## Client-owned non-blocker

Telegram Android composer "three dots" behavior is client-owned. Xiao may document it but must not fabricate a server-side fix.
