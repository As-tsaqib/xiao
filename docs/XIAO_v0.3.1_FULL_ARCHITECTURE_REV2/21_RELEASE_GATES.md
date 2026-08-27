# 21 — Release Gates

## P0 must pass

- no pre-main auxiliary semantic LLM call;
- photo/vision Unknown route works;
- one permanent Telegram final;
- `/new` preserves AI binding;
- model one-tap does not require probe;
- no unrestricted root/shell escape;
- PR #5 functional regressions are not merged;
- WebUI/daemon action contracts pass integration tests.

## P1 must pass

- mixed scheduler integrated;
- production plan/script cache use proven;
- delivery ACK shared across frontends;
- first-byte/first-visible timing recorded;
- capability overrides exposed in WebUI;
- GenerationCompleted timeline fixed;
- long/multilingual PDF behavior truthful;
- no-progress repair test.

## Automated gate

Exact final SHA:
- rustfmt PASS;
- cargo check PASS;
- cargo test PASS;
- strict clippy PASS;
- release build PASS;
- WebUI build PASS;
- Android arm64 PASS;
- deterministic module ZIP PASS.

## Device gate

All mandatory gates A–O in `20_REAL_DEVICE_ACCEPTANCE.md`, except P is informational.

## Version promotion

Only after gates pass:
- `Cargo.toml` → `0.3.1`;
- README/about/version metadata → `0.3.1`;
- validation doc records exact final SHA and CI run;
- real-device evidence appended;
- no stale "all pass" claim before evidence exists.
