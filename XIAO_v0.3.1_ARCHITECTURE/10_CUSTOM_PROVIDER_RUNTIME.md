# 10 — Custom Provider Runtime

## One active provider family

v0.3.1 continues the Custom-only runtime direction. Do not add Codex or Antigravity flows back into Telegram or active ProviderRegistry.

## Legacy cleanup

The baseline still carries legacy Codex/Antigravity config/types/adapters for compatibility. Clean this up without breaking history migration:

- legacy sessions remain readable;
- attempts to generate from a legacy session remain blocked until a Custom profile/model is selected;
- active runtime registry contains only Custom;
- user-facing config serialization should stop emitting unused legacy provider settings where safely migratable;
- legacy migration parsing may live in a clearly named `legacy` module rather than normal runtime paths;
- Custom-only installations must not fail config validation because an unused Antigravity URL is invalid.

Do not delete migration capability before old installations can be upgraded safely.

## Exact model/profile resolution

Every request resolves:

```text
session
 → exact Custom profile id
 → exact model id
 → endpoint
 → protocol
 → merged safe/secret headers
 → profile-owned API key
 → effective capability states
```

No provider-wide API key fallback.

## Multimodal serialization

Keep protocol-specific encoding behind provider adapter:

- Chat Completions: user content part with image data URL;
- Responses-compatible: input image content part;
- file input only when effective capability allows/tries it.

A capability error is normalized before the agent layer sees it.

## Streaming

Provider adapter owns SSE parsing and canonical event emission. AgentEngine should not parse provider-specific SSE.

## Structured fallback

Retain strict structured JSON fallback for endpoints without native tools, with existing transcript bounds. Improve long-run compaction so 150-turn ceiling cannot create unlimited structured transcript growth.

## Capability probe

One probe request suite may test:

- native tools;
- structured output;
- continuation;
- vision positive evidence;
- file input positive evidence;
- streaming handshake/support.

Probe failures are categorized; capability metadata must never be inferred from human-readable evidence strings.
