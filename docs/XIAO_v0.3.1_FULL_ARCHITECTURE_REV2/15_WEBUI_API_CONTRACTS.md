# 15 — WebUI API Contracts

The WebUI must use daemon contracts exactly. Presentation labels may differ from wire verbs.

## Resources

GET:
- dashboard
- telegram
- providers
- agent
- runtime
- context
- sessions
- runs
- attachments
- memory
- skills
- tools
- security
- diagnostics
- logs

POST:
- telegram
- provider-custom
- agent
- sessions
- runs
- attachments
- memory
- skills
- security

## Canonical actions

### Session AI
```json
{
  "action": "ai_config",
  "session_id": "...",
  "provider": "custom",
  "account_or_profile_id": "...",
  "model": "..."
}
```

### Attachment remove
```json
{
  "action": "remove",
  "attachment_id": "..."
}
```

### Memory delete
UI label: "Forget"

Wire:
```json
{
  "action": "delete",
  "scope": "user|memory",
  "category": "...",
  "key": "..."
}
```

### Skill delete
```json
{
  "action": "delete",
  "skill_id": "..."
}
```

### Custom profile create
```json
{
  "action": "create",
  "alias": "...",
  "endpoint": "...",
  "protocol": "openai_chat_completions|openai_responses",
  "headers": {"X-Workspace":"..."},
  "secret_headers": {"Authorization":"..."},
  "api_key": "optional"
}
```

### Custom profile edit
```json
{
  "action": "edit",
  "profile_id": "...",
  "alias": "...",
  "endpoint": "...",
  "protocol": "...",
  "headers": {},
  "secret_headers": {},
  "api_key": "...",
  "remove_api_key": false,
  "clear_secret_headers": false,
  "keep_credential": false,
  "keep_safe_headers": false,
  "keep_secret_headers": false
}
```

Endpoint change clears old trust material by default. "Keep" is explicit.

### Capability override
```json
{
  "action": "capability_override",
  "profile_id": "...",
  "model": "...",
  "capability": "vision|file_input",
  "owner_override": "auto|force_supported|force_unsupported"
}
```

### Probe
```json
{
  "action": "probe",
  "profile_id": "...",
  "model": "..."
}
```

Probe is optional and never required for selection.

## Refresh mapping

UI page IDs must map to data resource IDs:

```text
models        → providers
profiles      → providers
profile-edit  → providers
telegram      → setup/telegram
runs          → tasks/runs
session-detail→ sessions
...
```

Implement a typed reload map rather than `load(sub)`.

## Secret rules

- never render API key/secret header values;
- never put secrets in URL/query/history state/logs;
- history state may contain IDs and safe redacted metadata only;
- localStorage contains appearance/navigation preferences only, never provider secrets.
