# Xiao v0.2.7 Validation Record

This record tracks the release-gated validation for Xiao v0.2.7.

The implementation is validated in GitHub Actions because the local artifact environment does not include a Rust toolchain or a rooted Android device. Rust, lint, test, release-build, Android cross-build, deterministic module packaging, JavaScript syntax, and static acceptance evidence are recorded here once the corresponding gates pass. Physical rooted-device execution is reported separately and is never inferred from cross-compilation.

## Pre-release evidence

- `cargo fmt --all -- --check`: PASS.
- `cargo check --locked --all-targets --all-features`: PASS.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: PASS with Rust 1.98.0.
- The prior `cargo test --locked --all-targets --all-features` diagnostic reached 225/226 library tests. Its sole failure was the WebUI typed Custom-provider boundary assertion; that production UI fix is now persisted on the branch and awaits the clean full-suite rerun recorded below.
- ShellCheck for module, Termux wrapper, device test, packaging and acceptance scripts: PASS.

Release version metadata remains at 0.2.6 until the full Rust suite, host release build, JavaScript/static acceptance, Android arm64 cross-build, deterministic package verification, and workflow/governance audit all pass.
