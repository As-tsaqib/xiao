# Xiao v0.3.1 — Master Specification, Revision 2

## Mission

Complete Xiao v0.3.1 as a fast, reliable, single-owner Android AI agent whose observed responsiveness is close to the selected model/provider itself. The release must harden multimodal capability routing, stream visible model output with correct Telegram draft semantics, execute independent read-only work concurrently, batch safe Termux workflows, move learning out of the foreground, and make the WebUI a faithful control plane rather than a parallel source of truth.

The release is not a broad feature expansion. Do not add MCP, subagents, remote nodes, autonomous cron, vector DB, browser automation platform, unrestricted root shell, or a plugin marketplace.

## User-facing goals

1. A simple informational prompt must reach the selected provider immediately. Xiao must not make a hidden classifier/verifier LLM call before the main generation request.
2. Fast models must feel fast in Telegram. Visible SSE deltas should arrive in an ephemeral draft where configured.
3. Image input from Telegram must reach a model that is actually capable of vision even when capability metadata started as Unknown.
4. Unknown capability must never be silently collapsed to Unsupported.
5. A model selection must be one-tap. Exact capability probing is optional and must not block activation.
6. `/new` creates a fresh conversation while preserving the current AI binding; YOLO resets off.
7. `/provider` selects the provider/profile/account; `/model` selects a model within that binding.
8. Routine independent read-only tool calls should execute concurrently while preserving deterministic result order.
9. Reusable multi-step Termux work may be expressed as one structured `termux_job` without bypassing policy.
10. Learning and skill synthesis happen only after the final frontend delivery is acknowledged.
11. WebUI must expose the actual runtime state, including model capability overrides and run latency.
12. Real rooted Android acceptance is mandatory before version promotion.

## Product identity

Xiao remains:

- single-owner;
- private/on-device control plane;
- Telegram-first conversational agent;
- CLI-manageable;
- WebUI-configurable;
- rooted-Android-oriented;
- single `xiaod` runtime authority;
- Termux for ordinary CLI execution;
- typed Android broker for privileged device operations.

## Release scope

### Required
- deterministic-first agent preflight;
- evidence-first completion;
- provider streaming for supported Custom protocols;
- canonical stream events;
- tri-state capability evidence and owner overrides;
- Telegram image reliability;
- scanned PDF fallback chain;
- mixed-batch read-only scheduler;
- `termux_job` hardening;
- production plan/script cache use;
- post-delivery background learning;
- `/new`, `/login`, `/provider`, `/model` behavior fixes;
- result-aware no-progress logic;
- full Telegram live execution timeline;
- Android Unicode emoji fallback;
- WebUI PR #5 visual improvements ported without functional regressions;
- run latency telemetry;
- CLI/frontend delivery acknowledgement;
- real-device acceptance.

### Explicitly deferred
- MCP;
- vector DB;
- multi-agent/subagent delegation;
- remote device nodes;
- unrestricted root shell;
- autonomous background missions;
- generic browser automation platform;
- dynamic native plugin marketplace.

## Release principle

A green CI run proves build/test integrity, not device correctness. The final release decision requires both:

```text
automated exact-head CI
        +
rooted Android acceptance evidence
        =
v0.3.1 release-ready
```
