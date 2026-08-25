# Xiao v0.3.1 validation record

This record is intentionally incomplete until the exact-head CI and rooted Android gates are run. It must not be used as evidence that real-device validation occurred.

## Automated

- Branch: `feat/v0.3.0-single-binary`
- Schema migration: 26 → 27
- Exact SHA/run: 0f7d4681a505963547c99a6d379ae9243bef4d38 (Run 32910451727)
- Device screenshot proves ProgressIcon::Thinking custom emoji ID 5535034915403333642 is clipped at bottom by Telegram line box; fixed by defaulting to Unicode fallback 💭.
- Local Rust, WebUI, and Android builds/tests: not run (repository policy).

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

Required device metadata and measured timings from `XIAO_v0.3.1_ARCHITECTURE/20_REAL_DEVICE_ACCEPTANCE.md` must be recorded here by the device operator before release readiness. PR #2 remains draft and must not be merged or marked ready based on CI alone.
