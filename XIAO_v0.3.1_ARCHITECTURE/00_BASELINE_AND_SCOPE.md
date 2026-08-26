# 00 — Baseline and Scope

## Current repository state to preserve

The v0.3.1 work begins from the current v0.3.0 single-binary implementation, not from the older v0.2.x architecture.

Baseline facts at architecture creation:

```text
repo                 As-tsaqib/xiao
PR                   #2
branch               feat/v0.3.0-single-binary
baseline head        be8ccfb204e9ba512c6801f08af4ef2ef607b4e6
single executable    xiao
runtime mode         xiao daemon
active provider      custom
schema               26
exact-head CI        run #212 SUCCESS
```

Already-correct work that v0.3.1 must preserve unless implementation evidence proves a defect:

- one shipped native executable named `xiao`;
- `xiao daemon` remains the always-on runtime mode;
- one live mutable-state writer and local control socket architecture;
- Custom provider is the only active normal runtime provider;
- Telegram public command simplification from current v0.3 branch;
- per-session Termux workspaces;
- structured Termux argv execution rather than arbitrary model-supplied shell strings;
- typed Android/root broker boundary;
- single-owner authorization model;
- attachment lifecycle, scanned PDF paths, and cancellation lineage already established in v0.3;
- full live Telegram progress timeline and semantic progress icons;
- learned-skill system and canonical USER/MEMORY model;
- secrets remain outside plaintext config and UI output.

## Primary v0.3.1 goals

1. Fix false-negative multimodal routing.
2. Set the default agent turn ceiling to 150 and make it configurable in WebUI.
3. Reduce provider round trips and orchestration latency.
4. Implement true streaming for Custom endpoints.
5. Remove nonessential semantic LLM calls from the foreground response path.
6. Parallelize independent read-only tool calls safely.
7. Add a structured multi-step Termux execution tool so one provider tool call can accomplish a bounded workflow.
8. Add reusable plan/script caching without creating a policy bypass.
9. Add timing telemetry sufficient to prove where latency occurs.
10. Preserve truthful completion verification and safe cancellation.

## Non-goals for v0.3.1

Do NOT add:

- MCP;
- subagents or multi-agent delegation;
- vector DB;
- browser automation platform;
- remote device nodes;
- cron/autonomous missions;
- plugin marketplace;
- unrestricted root shell;
- new provider families;
- new Telegram management slash commands for runtime settings.

PicoClaw sub-turns and ZeroClaw multi-agent ideas are explicitly out of scope. v0.3.1 borrows their loop/streaming/tool-execution patterns, not their multi-agent scope.

## Versioning rule

The architecture package is named v0.3.1, but the implementation must not blindly bump the repository version at the start. Keep the working branch coherent, implement and validate all gates, then change package/version documentation to `0.3.1` only after release gates are green.
