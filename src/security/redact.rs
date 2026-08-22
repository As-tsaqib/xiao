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
            }
        }
    }
    if out.to_ascii_lowercase().starts_with("bearer ") {
        return "Bearer <redacted>".into();
    }
    out
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
}
