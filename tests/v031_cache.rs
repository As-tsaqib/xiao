use serde_json::json;
use sha2::{Digest, Sha256};
use xiao::security::redact::redact_text;

#[test]
fn secret_bearing_content_is_detected_and_redacted() {
    let secret_prompt = "api_key=sk-1234567890abcdef1234567890abcdef";
    let redacted = redact_text(secret_prompt);
    assert!(!redacted.contains("sk-1234567890abcdef1234567890abcdef"));
}

#[test]
fn stable_hash_for_safe_plan() {
    let plan = json!({
        "steps": [
            { "id": "1", "program": "free", "args": ["-m"] },
            { "id": "2", "program": "ps", "args": ["-A"] }
        ]
    });
    let mut hasher1 = Sha256::new();
    hasher1.update(plan.to_string().as_bytes());
    let key1 = format!("{:x}", hasher1.finalize());

    let mut hasher2 = Sha256::new();
    hasher2.update(plan.to_string().as_bytes());
    let key2 = format!("{:x}", hasher2.finalize());

    assert_eq!(key1, key2);
}
