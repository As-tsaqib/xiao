# 03 — Multimodal Capability Routing

## Baseline defect

The current Custom capability probe performs a tiny OCR challenge. A failed OCR challenge becomes `Unknown`; persisted model projection sets `vision_capable=false` unless the probe is exactly `Supported`; later the agent hard-rejects any image when `provider_capabilities.vision == false`.

This creates a false-negative path:

```text
multimodal model
  ↓
small probe OCR fails / endpoint behaves differently
  ↓
vision = Unknown
  ↓
stored bool = false
  ↓
real user image rejected locally
  ↓
model never receives image
```

## New state model

Retain tri-state as the primary semantic state and add owner override + evidence metadata.

```rust
enum CapabilityState {
    Supported,
    Unsupported,
    Unknown,
}

enum CapabilityOverride {
    Auto,
    ForceSupported,
    ForceUnsupported,
}

enum CapabilityEvidenceSource {
    ProbeSuccess,
    RuntimeSuccess,
    ProviderExplicitUnsupported,
    OwnerOverride,
    Migration,
}
```

Per exact `(profile_id, model_id, protocol)` store:

- `vision_state`;
- `file_input_state`;
- optional override;
- probe status/version/time;
- last runtime capability success time;
- last explicit unsupported time/error class;
- streaming capability state.

## Effective state precedence

```text
ForceSupported                         → Supported
ForceUnsupported                       → Unsupported
runtime confirmed success              → Supported
explicit provider unsupported evidence → Unsupported
probe supported                         → Supported
otherwise                               → Unknown
```

A probe failure alone MUST NOT create `Unsupported`.

## Runtime image routing

```text
incoming image
   ↓
normalize + quota + mime validation
   ↓
resolve effective vision state
   ├─ Supported   → send image
   ├─ Unknown     → optimistic one-shot send
   └─ Unsupported → clear user-facing error
```

On optimistic `Unknown`:

- successful multimodal response → persist `RuntimeSuccess` and state `Supported`;
- normalized explicit unsupported input error → persist `Unsupported`;
- 401/403 → auth error, state unchanged;
- 408/429/5xx/network timeout → transient, state unchanged;
- malformed textual answer → state unchanged;
- inability to OCR a specific image → state unchanged.

## Normalize provider errors

Add a provider adapter error class instead of scraping arbitrary human prose:

```rust
enum ProviderRequestErrorKind {
    UnsupportedCapability { capability: CapabilityKind },
    InvalidRequest,
    Authentication,
    RateLimited,
    Transient,
    Protocol,
    Other,
}
```

Only `UnsupportedCapability(ImageInput)` may automatically persist vision `Unsupported`.

## Probe redesign

The exact-model probe remains useful but becomes positive-evidence oriented.

- Use a sufficiently large high-contrast image (for example 320×160 or larger), not a 5×7 OCR font.
- Test image transport/schema acceptance and simple visual content.
- Successful response = Supported.
- HTTP/protocol rejection explicitly saying image input is unsupported = Unsupported.
- Model content mismatch, timeout, or generic failure = Unknown.

The probe's job is **capability evidence**, not a perfect benchmark of OCR skill.

## WebUI overrides

Per exact model expose:

```text
Vision       [ Auto | Force supported | Force unsupported ]
File input   [ Auto | Force supported | Force unsupported ]
```

Show separately:

- detected/effective state;
- override;
- evidence source;
- last probe/runtime confirmation;
- exact model/profile/protocol.

Changing endpoint or protocol invalidates prior automatic capability observations. Explicit owner overrides may be preserved only after explicit confirmation or reset to Auto by default.

## Acceptance

A model with `vision_state=Unknown` must be allowed to receive a real user image. CI must include a mock Custom endpoint where the probe is intentionally inconclusive but the real image request succeeds; Xiao must answer and persist Supported afterwards.
