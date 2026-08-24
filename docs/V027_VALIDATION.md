# Xiao v0.2.7 Validation Record

This record tracks the release-gated validation for Xiao v0.2.7.

The implementation is validated in GitHub Actions because the local artifact environment does not include a Rust toolchain or a rooted Android device. Rust, lint, test, release-build, Android cross-build, deterministic module packaging, JavaScript syntax, and static acceptance evidence are recorded here once the corresponding gates pass. Physical rooted-device execution is reported separately and is never inferred from cross-compilation.
