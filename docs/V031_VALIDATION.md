# Xiao v0.3.1 validation record

This record captures automated CI verification status and documents device limitations and boundaries. Manual rooted Android device gates remain unverified in this headless workspace.

## Automated

- Branch: `main`
- Schema migration: 26 → 27
- Verified CI Run: 33050274827 (Commit `79ea04ea5768800be7444747eb4bb73aa1e428c0`)
- `rust` job: PASS (cargo check, cargo test with 320 unit/integration tests passing, strict clippy, release build, WebUI build, static acceptance).
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
3. **Termux Job & Execution Plan Controls**:
   - `execution_plan_enabled` controls `termux_job` exposure and runtime execution.
   - Rejects approval-requiring substeps before execution with `approval_required` status and actionable guidance.
   - Multi-step inspections run in a single provider round trip with structured substep audit evidence and bounded aggregated results.
4. **Structured Plan & Script Cache Security**:
   - Safe structured plan caching with schema and environment fingerprinting; secret-bearing plans rejected.
   - `CachedScript` hash verification, trusted interpreter allowlist, and root denial.
5. **Multimodal Capability Matrix**:
   - Complete tri-state capability probing, exact image-schema error detection, transient failure preservation, and ForceSupported/ForceUnsupported enforcement.
6. **Streaming & Frontend SSE**:
   - Native tool-call delta assembly across Chat Completions and Responses protocols, reasoning token suppression, and no-retry on partial visible output.
7. **Foreground Latency & Learning Order**:
   - Deterministic verification avoiding redundant LLM calls, delivery acknowledgment before background learning release.
8. **Parallel Tool Concurrency & Interruption**:
   - Concurrent read-only execution with barrier ordering and durable interrupted state rows on cancellation.
9. **Storage & Migration 26→27**:
   - Idempotent migration preserving sessions, memories, skills, and profiles; production learning payloads survive restart and stale lease recovery.
10. **Observability**:
    - Real elapsed durations recorded using durable agent run start times for `final_frontend_delivery` and `background_learning`.
11. **WebUI Redesign & Daemon IPC Alignment**:
    - Functional `SessionAiDialog` and `ProfileEditor` with complete Custom profile lifecycle and write-only secret security.
    - Fully typed daemon IPC action routing for session AI (`ai_config`), custom profile discovery/probe (`test`/`probe`), attachment removal (`remove`), memory forget/reconcile, and approval decisions.

12. **CLI Hardening & End-to-End Contract Assurance**:
    - Universal pre-daemon syntax validation across all 22 command families and leaf subcommands.
    - Terminal-native subcommand help and `--help` resolution without requiring active daemon IPC.
    - Strict arity, option parsing, and exit code contract enforcement (0 ok, 1 error, 2 usage, 3 daemon unavailable, 4 rejected, 5 not found, 6 local io).
    - Global flags (`--json`, `--quiet`, `--session`, `--timeout`) validated with stable structured JSON error envelopes.
    - Strict security contract: zero secret leakage in argv, stdin/file-based secret ingestion, and sanitization of tokens/keys in CLI JSON responses.

13. **Tool Execution Reliability & False-Positive Loop Elimination**:
    - Resets consecutive identical failure counts and cleared failed action history on successful tool execution, preventing false-positive blocks when repairing intermediate failures.
    - Dynamic observation signature history buffer matching configured `max_no_progress_repeats`.
    - Parameter aliases for `pdf_create` (`file_path`, `filename`, `output_path`, `target`, `text`, `body`, `data`, `heading`, `header`) and `termux_terminal` (`command`, `cmd`, `argv`, `workdir`, `env`, `timeout`) ensuring first-attempt reliability across diverse LLM prompt patterns.

14. **Human-Readable CLI Output & Contract Hardening**:
    - Consistent, concise human-readable output by default with labeled sections, key-value rows, and bounded tables across all CLI commands (`status`, `context`, `doctor`, `tools`, `sessions`, `telegram`, `model`, `runs`, `attachments`, `memory`, `skills`, `approvals`, `config`, `daemon`, `logs`, `chat`).
    - Raw JSON, Telegram View blocks/actions, secrets, auth tokens, internal envelopes, and prompt/reasoning data strictly stripped from human output and CLI DTO projections.
    - Identifiers displayed only when necessary for follow-up operations.
    - `--json` guarantees stable `{"status":"ok", "data": ...}` machine envelope.
    - `--quiet` strictly suppresses stdout on success while preserving actionable stderr errors.
    - Decorative `--no-color` removed; `xiao help advanced` added for hidden plumbing/admin commands with prioritized help listing common commands first.
    - Comprehensive snapshot and contract tests enforcing zero secret leakage and absence of raw JSON blocks in human rendering.

15. **Secret Store Path Hardening & Mutation Projection Polish**:
    - `SecretStore::path` sanitizes keys to prevent path traversal or non-standard filesystem character injection.
    - CLI `telegram set-token-file`, `set-owner`, and `configure` responses route through `dto_telegram` / `project_telegram`, providing accurate human-readable status rendering after mutations.
    - Attachment ingestion transparently handles `file://` URI prefixes alongside relative and absolute filesystem paths.

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
