# 04 — Multimodal Capability Routing

## Capability dimensions

Per exact `(profile_id, model_id, protocol)` track:

- text
- native tool calls
- structured output
- continuation
- vision
- file input
- streaming

Optional capability states:

```text
Supported
Unsupported
Unknown
```

Agent protocol readiness is tracked separately from optional vision/file/streaming capability.

## Owner override

Per optional capability:

```text
Auto
ForceSupported
ForceUnsupported
```

Effective state:

```text
ForceSupported   → attempt capability
ForceUnsupported → reject capability
Auto             → use durable automatic evidence
```

Unknown under Auto may still be attempted when the request can safely establish evidence.

## Runtime evidence

### Upgrade to Supported
Only after an exact successful request demonstrating the capability.

Examples:
- image bytes were included and provider returned a valid answer;
- PDF file input was accepted and returned extracted content;
- SSE returned valid visible delta(s).

### Downgrade to Unsupported
Only on explicit capability/schema/provider rejection that is specific enough to prove unsupported.

### Stay Unknown
- timeout;
- DNS/network failure;
- 429;
- 5xx;
- auth failure;
- malformed unrelated response;
- cancellation.

## Model selection

Selection is never blocked by an optional capability probe.

One-tap selection:
1. activate exact profile/model;
2. display known capability states;
3. if Unknown, attempt safely when needed;
4. persist resulting evidence.

`Probe` remains an explicit optional button/command.

## WebUI requirements

Each model detail must show:

```text
Agent protocol: Native / Structured / ChatOnly / Indeterminate
Vision:        Supported / Unsupported / Unknown
File input:    Supported / Unsupported / Unknown
Streaming:     Supported / Unsupported / Unknown

Override:
Vision      [Auto | Force Supported | Force Unsupported]
File input  [Auto | Force Supported | Force Unsupported]
Streaming   [Auto | Force Supported | Force Unsupported]
```

At minimum Vision and File Input override must be present in v0.3.1. Streaming override may be omitted if not yet persisted as an owner override, but state must still be visible.

## Invalidation

Changing endpoint or protocol:
- clear automatic evidence;
- clear discovered model metadata when necessary;
- reset reachability;
- do not silently reuse credentials across trust boundary unless owner explicitly requested keep;
- do not inherit another profile's capability evidence.
