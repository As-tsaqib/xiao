# 10 — Sessions, Login, Provider, Model Commands

## `/new`

Bugfix invariant:

A fresh session must not lose the current AI configuration.

```text
old session:
provider/profile/model = current binding
YOLO = maybe on

/new
  ↓
new session:
provider/profile/model = inherited
YOLO = OFF
messages = empty
```

Global USER/MEMORY/skills persist.

## `/login`

Custom profile onboarding:
1. endpoint;
2. optional API key;
3. alias;
4. save;
5. model discovery may run after profile creation but must not block basic profile creation.

Alias collision resolution:
- requested `custom` and existing `custom` → `custom_1`;
- then `custom_2`, etc.;
- deterministic smallest free suffix;
- never overwrite an existing profile.

## `/provider`

Purpose: choose provider/profile/account for the current session.

It does not choose a model implicitly unless the selected profile has exactly one usable default and product behavior explicitly documents it.

For current Custom-first product:
- list Custom profiles;
- show current profile;
- one tap selects profile and preserves current model if valid, otherwise chooses configured default or asks for model.

## `/model`

Purpose: choose exact model under current provider/profile.

Requirements:
- one-tap activation;
- max 5/page;
- no mandatory exact probe before activation;
- show readiness/capability hints without blocking;
- optional `Probe` action separate from selection.

## Separation

Do not make `/model` silently change provider/profile.
Do not make `/provider` a duplicate `/model` screen.

## Session-specific binding

Provider/profile/model belongs to session.
Changing it in one session does not silently mutate another session.

WebUI must follow the same rule: a model page must not silently pick "first session" as a target.
