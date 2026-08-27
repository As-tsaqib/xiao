# 01 — Hard Invariants

## Ownership

- Exactly one owner identity exists for the installation.
- Telegram `owner_user_id` is an authentication binding, not a new durable memory namespace.
- `allowed_chat_ids` restrict where the owner can use Xiao; it never defines ownership.
- USER, MEMORY, skills, provider profiles, and durable owner state remain owner-global.

## Conversation scope

- Telegram scope is `(chat_id, message_thread_id)`.
- Sessions in different Telegram topics never leak or appear in each other's manager.
- Side chat is isolated from main history but may read parent context.
- New main session resets YOLO off but inherits the current provider/profile/model binding.

## Foreground latency

- No auxiliary provider/LLM call is allowed before the first main provider request for ordinary user prompts.
- Local deterministic classification is allowed.
- Semantic interpretation is a bounded post-main-generation fallback only when hard evidence is insufficient.
- Informational prompts must not expose side-effect execution tools unless the user explicitly asks for action.

## Completion

- Model text cannot prove an action succeeded.
- Action completion requires observable evidence.
- Inspection may succeed from direct image input when the model actually received the image and no contradictory tool state exists.
- Semantic completion cannot manufacture evidence or override DENY/approval state.

## Multimodal truth

Capability is one of:

```text
Supported | Unsupported | Unknown
```

Owner override is one of:

```text
Auto | ForceSupported | ForceUnsupported
```

- Unknown is not false.
- ForceSupported permits an attempt; it does not fabricate successful evidence.
- Explicit provider unsupported errors may change automatic evidence to Unsupported.
- Transient/network/auth failures must not convert capability to Unsupported.
- Endpoint/protocol/model/profile changes invalidate automatic evidence while preserving explicit owner override only when its scope is still valid.

## Provider/model selection

- Model activation never waits on an exact capability probe.
- Probing is optional, explicit, and bounded.
- Runtime success may upgrade Unknown to Supported.
- `/provider` and `/model` remain separate concerns.

## Attachments

- Telegram media is persisted under Xiao's private attachment root before provider use.
- Provider input is built from the stored attachment, not from an untrusted temporary external path.
- Xiao never claims to have read an image/PDF that never reached a successful local/provider processing path.
- Cancellation propagates through attachment rendering/OCR/provider fallback.

## Streaming

- Hidden reasoning/chain-of-thought is never streamed or persisted.
- Visible user-facing text deltas may be streamed.
- Tool-call argument deltas are assembled internally, not displayed as text.
- After any visible partial output, protocol fallback must not duplicate the response.
- Exactly one permanent final answer is sent per successful Telegram run.

## Tool execution

- Read-only calls may run concurrently.
- Side-effect, sensitive, destructive, privileged, or dependency-mutating steps are barriers unless explicitly proven independent and policy-safe.
- Result ordering is deterministic and follows original provider call order.
- A tool is never run merely because a compatibility alias bypassed canonical policy.

## Termux/root boundary

- `bash -c`, `sh -c`, opaque command strings, `su`, `tsu`, `sudo`, and `doas` from model-supplied execution are hard denied.
- Routine structured Termux work may run without owner approval when it is inside the allowed workspace/capability boundary.
- Destructive filesystem operations are auto-allowed only when all targets resolve inside the session workspace; otherwise ASK or DENY.
- Privileged Android mutation goes through typed AndroidBroker.
- YOLO converts eligible ASK to ALLOW only for the active session; hard DENY remains DENY.

## Learning

- Positive memory/skill learning only follows VerifiedSuccess.
- Background learning cannot start until final frontend delivery is acknowledged.
- Failed/cancelled/blocked runs do not become positive procedures.
- Memory is current canonical state, not append-only preference duplication.

## WebUI

- xiaod is the source of truth.
- The WebUI cannot invent unsupported protocols/actions/settings.
- Visual rewrites must preserve working manager API contracts.
- Generated `module/webroot` assets are build output; `webui/src` is source of truth.
- No remote runtime CSS/JS dependency is required for the manager to render offline.
- Secret values are write-only.

## Release

- Version must remain pre-0.3.1 until exact-head CI and rooted Android gates pass.
