# 14 — PR #5 Review

PR:
- `#5 feat(webui): complete WebUI overhaul with dark theme, safe-area insets, and smooth transitions`
- head: `85fee2b925172c0313632c4f9b557137bd646097`
- base: `080ac88aa7c65fe54c52bb2ecbd82712d03a432d`
- current main differs from that base only by a validation-doc commit.
- GitHub metadata currently reports `mergeable: false`.
- no workflow run/status was found for the PR head.

## Verdict

**Do not merge PR #5 as-is.**

The visual direction is useful, but `webui/src/App.jsx` in the PR replaces working control-plane logic with stale/incomplete contracts.

## Keep / port

Port these changes onto current `main` source:
- safe-area handling;
- dark palette;
- smooth touch feedback;
- optimistic toggles with rollback;
- Android back navigation concept;
- manual refresh affordance;
- full-width mobile layout if desired;
- refresh spinner/progress affordance.

## P0/P1 regressions in PR source

### 1. Session AI actions regress
PR uses:
```text
action: change_ai
```
Current daemon uses:
```text
action: ai_config
provider
account_or_profile_id
model
```

The PR also defines `SessionAiDialog()` as `return null`, effectively removing the working dialog.

### 2. Attachment delete action regresses
PR:
```text
action: delete
```
Daemon:
```text
action: remove
```

### 3. Memory action payload regresses
PR sends an ID-only delete shape. Daemon requires:
```text
action: delete
scope
category
key
```

### 4. Custom profile edit regresses
PR:
```text
action: update
safe_headers: raw JSON string
secret_headers: raw JSON string
```

Daemon:
```text
action: edit
headers: object
secret_headers: object
```

Raw `secret_headers` string will not match the expected map type.

### 5. Unsupported protocol options
PR offers:
- `anthropic_messages`
- `google_gemini`

Backend profile validation currently accepts only:
- `openai_chat_completions`
- `openai_responses`

UI must not advertise protocols the daemon rejects.

### 6. Profile trust-boundary controls are lost
Current main has endpoint-change keep/clear controls for API key, safe headers, and secret headers. PR simplifies the form and loses those semantics.

### 7. Manual refresh is route-incorrect
PR does:
```text
if sub: load(sub)
```

But UI subpage names and manager resources are not 1:1:
- `models` data comes from `providers`;
- `profiles` data comes from `providers`;
- `runs` page currently reads `data.tasks`;
- `telegram` page currently reads `data.setup`.

Manual refresh can call unsupported resources or update the wrong data key.

Use a page-to-resource/reload map.

### 8. Theme "system" mode is not actually persistent system mode
On first launch the PR reads system dark/light, then immediately stores that concrete value to localStorage. Future OS theme changes no longer follow system.

Use:
```text
themePreference = system | light | dark
```

### 9. Remote CSS dependency
PR source adds:
```css
@import url("https://mui.kernelsu.org/internal/insets.css");
```

while HTML already links:
```html
<link rel="stylesheet" href="/internal/insets.css">
```

The remote import creates an unnecessary network/supply-chain/privacy dependency and duplicates the local KernelSU path.

Keep local `/internal/insets.css`; remove remote import.

### 10. Safe-area hard minimum
`max(38px, ...)` forces a top gap even outside embedded Android. Prefer actual KernelSU/env inset with an embedded-only fallback.

### 11. Accessibility regression
PR hides bottom-tab labels and does not add per-button `aria-label`. Icon-only navigation must retain accessible names.

### 12. Reduced-motion regression
The original `prefers-reduced-motion` rule is removed while many new animations are added. Restore it.

## Merge strategy

Do not resolve this by choosing either "main App.jsx" or "PR App.jsx" wholesale.

Use:

```text
current main functional App.jsx / daemon contracts
        +
PR #5 visual tokens, theme, inset, animation, history ideas
        ↓
new integrated WebUI
        ↓
source build
        ↓
generated module/webroot
```

Never hand-edit compiled `module/webroot/assets/app.js` or `app.css` as the primary source.
