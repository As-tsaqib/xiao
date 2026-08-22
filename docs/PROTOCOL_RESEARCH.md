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

The module follows the current KernelSU module lifecycle: `module.prop`, `customize.sh`, `post-fs-data.sh`, `service.sh`, action/watchdog scripts, and `webroot/index.html` as WebUI entry point. WebUI is a thin root administration surface routed through `action.sh`; application state remains in the daemon and persistent `/data/adb/xiao` directory.

The JavaScript bridge is isolated in `module/webroot/assets/ksu-bridge.js` so changes in the KernelSU injected execution bridge can be updated without touching application logic.

## OpenAI Codex authentication and transport

Cross-checks against CLIProxyAPI commit
`0a14eb70ce19fac1d114bcdb4a476d61adc819e2` found its default Codex login uses
browser Authorization Code + PKCE with the Codex CLI client, localhost
callback, offline scope, organization claims, and simplified-flow flag. xiao
implements that contract in `AuthManager`; the ChatGPT Codex Responses
transport remains isolated in `CodexProvider`.

[Official OpenAI Codex authentication documentation](https://developers.openai.com/codex/auth/)
confirms that ChatGPT sign-in opens a browser, returns credentials through a
local callback, and uses `localhost:1455` for the standard browser flow. The
exact OAuth query/form compatibility parameters in this patch come from the
CLIProxyAPI source requested for this implementation.

At the implementation snapshot, CLIProxyAPI's Codex client catalog includes `gpt-5.6-sol` and retains `gpt-5.5` as a required template. xiao therefore uses those as a conservative fallback model list while still allowing config-selected defaults.

The callback adapter validates transaction state, extracts the ChatGPT account identifier from token claims, stores per-account refresh credentials, and refreshes under an account-scoped lock. The callback listener exists only while a login transaction is active and expires after five minutes.

## Antigravity authentication, project discovery, and models

CLIProxyAPI, 9Router, and OmniRoute were cross-checked rather than assuming a static “Gemini OAuth” flow. xiao's first compatibility patch now follows CLIProxyAPI's installed-app client, scopes, localhost callback, offline consent, userinfo lookup, `loadCodeAssist` project/tier discovery, and `onboardUser` fallback. Operators may override the installed-app client ID and keep its optional secret in `SecretStore`; the default needs no extra WebUI setup.

OmniRoute's active catalog on 2026-08-20 shows newer Antigravity IDs including Gemini 3.7 Flash tiers, `gemini-pro-agent` for the live Gemini 3.1 Pro High path, Gemini 3.1 Pro Low, and current Claude 4.6 IDs. xiao uses a small static fallback list of current callable IDs; the adapter boundary is intentionally the place to add authenticated live model discovery later without changing session/command/UI contracts.

## Custom provider

Custom transport assumptions are explicitly configured: OpenAI
Responses-compatible or Chat Completions-compatible. Base URL, models, and
non-secret headers are normal config. API keys are written only to SecretStore.
The root WebUI can query `{base_url}/models`, accepts the common OpenAI
`{"data":[{"id":"…"}]}` shape plus simple model arrays, sorts/deduplicates
IDs, and reuses a previously stored Bearer key without exposing it. The live
CLIProxyAPI module at `http://127.0.0.1:8317/v1` is the device E2E target.
