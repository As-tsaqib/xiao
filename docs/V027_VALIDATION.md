# Xiao v0.2.7 Validation Record

This record tracks the release-gated validation for Xiao v0.2.7.

The implementation is validated in GitHub Actions because the local artifact environment does not include a Rust toolchain or a rooted Android device. Rust, lint, test, release-build, Android cross-build, deterministic module packaging, JavaScript syntax, and static acceptance evidence are recorded here once the corresponding gates pass. Physical rooted-device execution is reported separately and is never inferred from cross-compilation.

## Pre-release evidence

- `cargo fmt --all -- --check`: PASS.
- `cargo check --locked --all-targets --all-features`: PASS.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: PASS with Rust 1.98.0.
- `cargo test --locked --all-targets --all-features`: 225/226 library tests passed on the latest diagnostic run; the remaining WebUI typed Custom-provider boundary regression is being corrected before the release gate is opened.
- ShellCheck for module, Termux wrapper, device test, packaging and acceptance scripts: PASS.

Release version metadata remains at 0.2.6 until the full Rust suite, host release build, JavaScript/static acceptance, Android arm64 cross-build, deterministic package verification, and workflow/governance audit all pass.
