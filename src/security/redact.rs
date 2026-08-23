const KEYS: &[&str] = &[
    "authorization",
    "access_token",
    "refresh_token",
    "id_token",
    "api_key",
    "api-key",
    "bot_token",
    "telegram-bot-token",
    "ipc-client-token",
    "ipc-admin-token",
    "code_verifier",
    "client_secret",
    "cookie",
    "password",
    "passcode",
    "secret",
];

pub fn redact_text(input: &str) -> String {
    let mut out = input.to_owned();
    for line in input.lines() {
        let lower = line.to_ascii_lowercase();
        if KEYS.iter().any(|k| lower.contains(k)) {
            if let Some(pos) = line.find([':', '=']) {
                let prefix = &line[..=pos];
                out = out.replace(line, &format!("{prefix} <redacted>"));
            } else {
                out = out.replace(line, "<redacted sensitive text>");
            }
        }
    }
    for token in input.split_whitespace() {
        let candidate = token.trim_matches(|character: char| {
            matches!(
                character,
                '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        });
        if looks_like_secret_token(candidate) {
            out = out.replace(candidate, "<redacted-token>");
        }
    }
    if out.to_ascii_lowercase().starts_with("bearer ") {
        return "Bearer <redacted>".into();
    }
    out
}

/// Redact structured values before audit persistence. Key matching is
/// case-insensitive and recursive so a one-line JSON object cannot bypass the
/// line-oriented log redactor.
pub fn redact_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let value = if KEYS.iter().any(|sensitive| lower.contains(sensitive)) {
                        serde_json::Value::String("<redacted>".into())
                    } else {
                        redact_json(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(redact_json).collect())
        }
        serde_json::Value::String(value) => serde_json::Value::String(redact_text(value)),
        value => value.clone(),
    }
}

/// Learned state is intentionally more conservative than surfaced logs: if a
/// value appears to contain credentials, the candidate is rejected entirely.
pub fn contains_secret_material(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if value.split_whitespace().any(|token| {
        looks_like_secret_token(token.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_' | '.' | ':')
        }))
    }) {
        return true;
    }
    if lower.contains("-----begin") && lower.contains("private key-----") {
        return true;
    }
    [
        "password",
        "passcode",
        "api_key",
        "api key",
        "access_token",
        "access token",
        "refresh_token",
        "refresh token",
        "bot_token",
        "bot token",
        "client_secret",
        "client secret",
        "private key",
    ]
    .iter()
    .any(|needle| credential_marker_has_value(&lower, needle))
        || lower.contains("bearer ")
}

fn credential_marker_has_value(value: &str, marker: &str) -> bool {
    let mut remainder = value;
    while let Some(index) = remainder.find(marker) {
        let before = &remainder[..index];
        let after = remainder[index + marker.len()..].trim_start_matches([' ', '"', '\'']);
        if after.starts_with([':', '='])
            || after.starts_with("is ")
            || before.trim_end().ends_with("my")
            || before.trim_end().ends_with("our")
        {
            return true;
        }
        remainder = &remainder[index + marker.len()..];
    }
    false
}

fn looks_like_secret_token(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let prefixed = ["sk-", "ghp_", "github_pat_", "xoxb-", "xoxp-", "AIza"]
        .iter()
        .any(|prefix| value.starts_with(prefix) || lower.starts_with(&prefix.to_ascii_lowercase()));
    if prefixed && value.chars().count() >= 12 {
        return true;
    }
    let jwt_parts = value.split('.').collect::<Vec<_>>();
    if value.starts_with("eyJ")
        && jwt_parts.len() == 3
        && jwt_parts.iter().all(|part| part.len() >= 8)
    {
        return true;
    }
    let Some((left, right)) = value.split_once(':') else {
        return false;
    };
    left.len() >= 6
        && left.chars().all(|character| character.is_ascii_digit())
        && right.len() >= 20
        && right
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

pub fn mask_token(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "********".into();
    }
    format!(
        "{}…{}",
        chars[..4].iter().collect::<String>(),
        chars[chars.len() - 4..].iter().collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hides_secret_values() {
        assert!(!redact_text("Authorization: Bearer abc123").contains("abc123"));
        assert!(!redact_text("my password is abc123").contains("abc123"));
    }
    #[test]
    fn hides_oauth_and_ipc_material() {
        for sample in [
            "id_token=header.payload.signature",
            "code_verifier: verifier-secret",
            "ipc-client-token=client-secret",
            "ipc-admin-token=admin-secret",
        ] {
            assert!(!redact_text(sample).contains("secret"));
        }
    }
    #[test]
    fn masks_token() {
        assert_eq!(mask_token("1234567890abcdef"), "1234…cdef");
    }

    #[test]
    fn structured_redaction_hides_nested_secret_values() {
        let redacted = redact_json(&serde_json::json!({
            "nested": {"api_key": "do-not-persist"},
            "safe": "visible"
        }));
        assert_eq!(redacted["nested"]["api_key"], "<redacted>");
        assert_eq!(redacted["safe"], "visible");
        assert!(!redacted.to_string().contains("do-not-persist"));
    }

    #[test]
    fn learned_state_secret_detector_is_conservative() {
        assert!(contains_secret_material("my API key is abc"));
        assert!(contains_secret_material("Bearer abc.def"));
        assert!(contains_secret_material(
            "remember sk-proj-1234567890abcdef"
        ));
        assert!(contains_secret_material(
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.signature123"
        ));
        assert!(!contains_secret_material("use a token budget of 4000"));
        assert!(!contains_secret_material(
            "Do not expose passwords or API keys from logs"
        ));
    }
}
