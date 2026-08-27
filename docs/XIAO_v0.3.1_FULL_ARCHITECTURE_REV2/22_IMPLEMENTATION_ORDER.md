# 22 — Implementation Order

1. **Protect current working behavior**
   - add contract tests for `/new`, `/login` alias suffix, `/provider`, `/model`, photo path, direct-final, no-progress, PDF, CLI.

2. **Fix foreground latency**
   - local deterministic preflight only;
   - evidence-first completion;
   - correct latency fixtures with semantic evaluation actually enabled.

3. **Integrate scheduler**
   - mixed read-only groups and barriers.

4. **Make cache real**
   - wire PlanCache/ScriptCache into production execution;
   - add telemetry.

5. **Refine Termux policy**
   - workspace-aware destructive targets;
   - trusted cached script path;
   - retain hard deny for root/opaque shell.

6. **Finish Telegram timeline**
   - GenerationCompleted finalization;
   - no synthetic activity;
   - Unicode fallback.

7. **Finish multimodal**
   - one-tap model selection;
   - optional probe;
   - runtime evidence;
   - image direct-inspection completion.

8. **Cross-frontend delivery**
   - shared delivery ACK service;
   - CLI ACK.

9. **WebUI integration**
   - start from current functional main App;
   - port PR #5 visual changes;
   - fix contracts;
   - capability overrides;
   - timing waterfall;
   - system theme;
   - safe-area local CSS;
   - back/refresh mapping;
   - accessibility/reduced motion.

10. **PDF reliability**
    - long/multilingual truthful behavior;
    - delivery tests.

11. **Exact-head CI**

12. **Rooted Android A–O**

13. **Version promotion**
