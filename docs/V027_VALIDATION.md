# Xiao v0.2.7 Validation Record

This record tracks the release-gated validation for Xiao v0.2.7.

The implementation is validated in GitHub Actions because the local artifact environment does not include a Rust toolchain or a rooted Android device. Rust, lint, test, release-build, Android cross-build, deterministic module packaging, JavaScript syntax, and static acceptance evidence are recorded here once the corresponding gates pass. Physical rooted-device execution is reported separately and is never inferred from cross-compilation.

## Pre-release evidence

- `cargo fmt --all -- --check`: PASS on the implementation before the final CLI integration additions; the complete tree is rerun below.
- `cargo check --locked --all-targets --all-features`: PASS on Rust 1.98.0 before the final CLI integration additions; the complete tree is rerun below.
- ShellCheck for module, Termux wrapper, device test, packaging and acceptance scripts: PASS.
- The Rust library suite reached 225/226 before the explicit WebUI session-manager parity helper was added. That sole parity assertion is fixed in the current tree.
- CLI binary integration tests, help snapshot, and architecture-aligned `status: ok|error` JSON envelope tests are now part of the current tree and must pass in the clean rerun.

Release version metadata remains at 0.2.6 until the complete Rust suite, strict clippy, host release build, JavaScript/static acceptance, Android arm64 cross-build, deterministic package verification, and workflow/governance audit all pass.
