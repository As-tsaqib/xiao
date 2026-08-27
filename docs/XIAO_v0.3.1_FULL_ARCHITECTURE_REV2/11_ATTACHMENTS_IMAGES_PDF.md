# 11 — Attachments, Images, PDF

## Private attachment lifecycle

Telegram download:
1. validate size/type/quota;
2. persist to Xiao private attachment root;
3. record metadata in SQLite;
4. normalize image/document as needed;
5. pass bytes/file representation to selected provider only when referenced;
6. cleanup by retention/session deletion rules.

Never depend on an ephemeral Telegram temp path after persistence.

## Telegram photo bug

For "Apa ini" with an attached photo:
- attachment status becomes `ready`;
- exact image bytes are included in provider request;
- completion treats direct visual inspection as valid inspection evidence;
- result must not be converted into Blocked solely because no tool side effect occurred.

## Capability routing

If vision state:
- Supported → send image.
- Unknown + Auto → optimistic exact request; learn from result.
- Unsupported/ForceUnsupported → reject with truthful capability message.
- ForceSupported → attempt exact request.

## Scanned PDF chain

```text
embedded text extraction
    ↓ if insufficient
local render + OCR
    ↓ if unavailable/insufficient
provider file_input if effective supported/unknown-safe
    ↓
provider vision page images if effective supported/unknown-safe
    ↓
chunk/index extracted text
    ↓
answer
```

If no path succeeds:
- status `needs_ocr` or Blocked with explicit evidence;
- never pretend PDF was read.

Cancellation propagates through all stages.

## PDF creation

`pdf_create` remains deterministic and workspace-relative.

Requirements:
- output path canonical under session workspace;
- parseable output;
- multi-page wrapping;
- artifact surfaced for frontend delivery;
- no symlink escape.

### Multilingual behavior

Silent character destruction is not acceptable.

For unsupported glyphs, either:
1. use a verified available Unicode font backend; or
2. return an explicit unsupported-glyph warning/error.

Do not silently ASCII-sanitize and claim full fidelity.

Real-device acceptance includes:
- long multi-page document;
- Indonesian punctuation/diacritics;
- at least one non-Latin sample;
- Telegram artifact delivery.
