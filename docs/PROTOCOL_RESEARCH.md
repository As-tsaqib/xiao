# Protocol research snapshot — 2026-08-22

xiao keeps these assumptions inside transport/auth adapters because all of them are upstream-controlled and can change independently of the Command Core.

## Telegram Bot API

Research was performed against the current official Telegram Bot API documentation (Bot API 10.2 at implementation time).

- Inline callback data remains limited to 1–64 bytes, which is why xiao stores menu state server-side and sends only compact menu/revision/action identifiers.
- Callback queries must be acknowledged quickly; xiao ACKs before waiting on the per-menu serialization lock or executing semantics.
- Rich Messages are available through `sendRichMessage`; ephemeral draft progress is available through `sendRichMessageDraft`. Reusing the same non-zero `draft_id` updates the draft. Drafts are not used as final history.
- `thinking` is draft-only in xiao. Final views intentionally drop progress blocks.
- Rich table/list/details block payloads are emitted using current structured block shapes rather than embedding Telegram-specific formatting into Command Core.

## KernelSU Next modules/WebUI

The module follows the current KernelSU module lifecycle: `module.prop`, `customize.sh`, `service.sh`, optional action script, and `webroot/index.html` as WebUI entry point. WebUI is a thin root administration surface over the module's fixed `xiao` binary; application state remains in the daemon and persistent `/data/adb/xiao` directory.

The JavaScript bridge is isolated in `webui/assets/ksu-bridge.js` so changes in the KernelSU injected execution bridge can be updated without touching application logic.

## OpenAI Codex authentication and transport

Cross-checks against current CLIProxyAPI implementation found the device authorization flow using OpenAI's device-auth endpoints and the ChatGPT Codex Responses transport. xiao localizes those endpoints/constants in `AuthManager`/`CodexProvider`; it does not expose them to the Command Core.

At the implementation snapshot, CLIProxyAPI's Codex client catalog includes `gpt-5.6-sol` and retains `gpt-5.5` as a required template. xiao therefore uses those as a conservative fallback model list while still allowing config-selected defaults.

The device polling adapter treats HTTP 403/404 as pending states, extracts the ChatGPT account identifier from token claims, stores per-account refresh credentials, and refreshes under an account-scoped lock.

## Antigravity authentication, project discovery, and models

CLIProxyAPI, 9Router, and OmniRoute were cross-checked rather than assuming a static “Gemini OAuth” flow. Current implementations use Google OAuth with offline access, followed by `loadCodeAssist` project/tier discovery and `onboardUser` when no companion project is already available. xiao follows that sequence while requiring the deployment's own Google Desktop OAuth client ID through normal daemon configuration/KernelSU WebUI. An optional OAuth client secret is stored only in `SecretStore`; it is never copied from a third-party project or returned unmasked.

OmniRoute's active catalog on 2026-08-20 shows newer Antigravity IDs including Gemini 3.7 Flash tiers, `gemini-pro-agent` for the live Gemini 3.1 Pro High path, Gemini 3.1 Pro Low, and current Claude 4.6 IDs. xiao uses a small static fallback list of current callable IDs; the adapter boundary is intentionally the place to add authenticated live model discovery later without changing session/command/UI contracts.

## Custom provider

Custom transport assumptions are explicitly configured: OpenAI Responses-compatible or Chat Completions-compatible. Base URL, models, and non-secret headers are normal config. API keys are written only to SecretStore. This avoids silently assuming every OpenAI-compatible server has identical auth or endpoint behavior.
