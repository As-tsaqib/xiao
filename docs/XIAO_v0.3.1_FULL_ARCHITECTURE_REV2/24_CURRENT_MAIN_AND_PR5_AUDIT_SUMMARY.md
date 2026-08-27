# Current Main + PR #5 Audit Summary

## Main baseline inspected
- branch: `main`
- exact audited head: `93f9f54255783c4e28fadbb1110f6620e30ade40`
- exact-head CI run observed: `33032382752`
- Rust and Android arm64 jobs: PASS

## Main remaining gaps
1. semantic-first pre-main task classification can add a hidden provider round trip;
2. mixed tool batch scheduler integration incomplete;
3. PlanCache/CachedScript are not yet proven production reuse;
4. Termux policy needs workspace-aware autonomy rather than program-name-only approval;
5. capability override UI missing;
6. first byte / first visible text delta not fully measured/displayed;
7. delivery-before-learning handshake is Telegram-centric;
8. GenerationCompleted adds synthetic "Finishing response";
9. rooted Android acceptance still pending.

## PR #5
- head `85fee2b925172c0313632c4f9b557137bd646097`
- useful visual direction;
- no CI run/status found for PR head;
- GitHub reports mergeable false;
- do not merge source wholesale.

Critical PR regressions:
- `change_ai` vs daemon `ai_config`;
- dummy `SessionAiDialog`;
- attachment `delete` vs `remove`;
- stale memory/profile action payloads;
- unsupported Custom protocol options;
- profile edit loses trust-boundary controls;
- manual refresh subpage/resource mismatch;
- system theme becomes a one-time snapshot;
- duplicate remote KernelSU inset import;
- reduced-motion rule removed;
- icon-only tabs lack accessible names.

Recommended approach:
port PR #5 appearance/UX changes onto current functional main and enforce integration tests against actual manager handlers.
