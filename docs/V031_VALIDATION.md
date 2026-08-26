# Xiao v0.3.1 validation record

This record captures automated CI verification status and documents device limitations and boundaries. Manual rooted Android device gates remain unverified in this headless workspace.

## Automated

- Branch: `main`
- Schema migration: 26 → 27
- Verified CI Run: 32977425340 (Commit `600a8d8973ec4ff132e635b319a2c0b6d13148f3`)
- `rust` job: PASS (cargo check, cargo test with 312 unit/integration tests passing, strict clippy, release build, WebUI build, static acceptance).
- `android-arm64` job: PASS (cargo-ndk build, WebUI embed, deterministic module ZIP build and sha256 checksum verification).
- Local Rust, WebUI, and Android builds/tests: not run (repository policy; all validation via GitHub Actions CI).

## Security & Implementation Milestones

1. **Termux Terminal Security Boundary**:
   - `termux_terminal` enforces strict per-session workspace canonicalization and containment.
   - Rejects absolute paths, path traversal (`..`), and symlink escapes in `cwd`.
   - Preflight inspection rejects sensitive program paths, cwd, argv tokens, and environment variables before invoking the executor.
2. **PDF Create Tool**:
   - `pdf_create` validates output paths strictly beneath the canonical session workspace without symlink escape.
   - Registered under safe side-effect policy only after containment verification.
   - Multi-page pagination and line wrapping are supported natively.
3. **Termux Job Approval Semantics**:
   - Explicitly rejects approval-requiring substeps before execution with `approval_required` status and actionable guidance (`unsupported inside termux_job; call termux_terminal separately for exact approval`).
4. **Capability & Stream Handling**:
   - Cached explicit `unsupported` streaming disables streaming on future requests while `unknown` remains optimistic.
   - Endpoint and protocol edits invalidate automatic evidence while preserving explicit owner capability overrides.
5. **Observability**:
   - Real elapsed durations recorded using durable agent run start times for `final_frontend_delivery` and `background_learning`.

## Device Limitations & Edge Behaviors

- **Telegram Android Client Styling**: Telegram Android clips multiple custom emoji inside `RichBlockThinking`, while iOS renders them fully. Draft progress uses Unicode fallbacks for cross-client consistency; completed `✓`/`✗` markers are unchanged. Composer icon styling and input controls are subject to Telegram Android client limitations and cannot be controlled via bot server draft payloads.
- **PDF Unicode & Fonts**: Built-in `pdf_create` generates standard PDF-1.4 documents with multi-page line wrapping and pagination using standard Type 1 Helvetica font metrics. Standard ASCII and Latin-1 text are rendered natively; complex non-Latin scripts fall back to sanitized ASCII representation to avoid bundled TTF font engine bloat on mobile.
- **Unprivileged Termux Sandbox**: `termux_terminal` executes strictly under the unprivileged Termux app UID with `PR_SET_NO_NEW_PRIVS` and dropped supplementary groups. Arbitrary root escalation is rejected; privileged Android operations route exclusively through the typed Android Broker with exact one-shot approval.
- **No-Progress Termination**: Agent turns are bounded by `max_turns` (default 150) and `max_no_progress_repeats` (3 repeats) to prevent infinite loops when tools encounter unresolvable environmental blockers.

## Rooted Android manual gates

All remain manual and unverified in this workspace:

- A: Unknown → Supported real Custom multimodal request
- B: explicit unsupported exact-model isolation
- C: Telegram visible SSE draft and single permanent final
- D: streamed tool continuation without protocol leakage
- E: controlled task exceeding eight provider turns
- F: overlapping read-only tool timing with stable order
- G: `termux_job` under the Termux UID with substep audit
- H: root escalation denial and typed broker approval/YOLO boundary
- I: `/stop` during SSE, parallel tools, and `termux_job`
- J: final delivery before background learning start

Required device metadata and measured timings from `XIAO_v0.3.1_ARCHITECTURE/20_REAL_DEVICE_ACCEPTANCE.md` must be recorded here by the device operator before release readiness. Automated verification runs directly on main; release readiness requires real-device evidence.
