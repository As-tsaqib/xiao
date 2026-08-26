use serde_json::json;
use xiao::{security::redact::redact_text, tools::cache::CachedPlan};

#[test]
fn secret_bearing_content_is_detected_and_redacted() {
    let secret_prompt = "api_key=sk-1234567890abcdef1234567890abcdef";
    let redacted = redact_text(secret_prompt);
    assert!(!redacted.contains("sk-1234567890abcdef1234567890abcdef"));
}

#[test]
fn stable_hash_for_safe_plan() {
    let plan = CachedPlan {
        steps: json!({
            "steps": [
                { "id": "1", "program": "free", "args": ["-m"] },
                { "id": "2", "program": "ps", "args": ["-A"] }
            ]
        }),
        schema_version: 1,
        environment_fingerprint: "termux-env".into(),
    };
    let key1 = plan.key().unwrap();
    let key2 = plan.key().unwrap();
    assert_eq!(key1, key2);

    let secret_plan = CachedPlan {
        steps: json!({
            "steps": [
                {
                    "id": "1",
                    "program": "echo",
                    "args": ["Authorization: Bearer sk-1234567890abcdef"]
                }
            ]
        }),
        schema_version: 1,
        environment_fingerprint: "termux-env".into(),
    };
    assert!(secret_plan.key().is_err());
}
