# Xiao v0.2.7 Validation Record

GitHub Actions is the authoritative Rust/Android validation environment for this release candidate.

## Pre-release gate

Candidate head `d6f8edd7f56efc472fca8fac7d493a4026e26ddd` passed run #134 (`32686544237`) on Rust 1.98.0: rustfmt, locked all-target/all-feature check and tests, strict clippy `-D warnings`, release build, ShellCheck, WebUI JavaScript syntax, static acceptance, Android arm64 cross-build, deterministic KernelSU ZIP/checksum verification, and artifact upload all PASS. The pre-version artifact was `xiao-v0.2.6-kernelsu-arm64` (artifact `9506162200`) and is evidence only, not the v0.2.7 release artifact.

## Workflow / governance audit

The final CI workflow uses `pull_request` plus `workflow_dispatch`, `contents: read`, `persist-credentials: false`, and full-SHA pinned reusable actions. It has no `pull_request_target`, secret consumption, self-push, or workflow mutation path. Source governance includes CODEOWNERS and SECURITY policy. Live repository metadata reports `main` unprotected with no required checks; the available connector exposes no branch-protection/ruleset mutation, so that external posture is recorded rather than falsely claimed fixed.

## Environment limitation

No rooted Android device, live Telegram account, or live provider credentials are available here. Physical-device/live-network acceptance is therefore unclaimed; deterministic fakes/regressions plus Android arm64 cross-build and deterministic packaging are used where supported.

The exact 0.2.7 release head must pass the same full CI matrix before the PR is marked ready for review.
