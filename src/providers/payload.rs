use serde_json::{json, Value};

use crate::storage::MessageRecord;

const OMITTED_USER_CONTEXT: &str = "(Earlier user context was omitted.)";

#[derive(Debug, Clone, PartialEq, Eq)]
struct WireMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedConversation {
    instructions: Option<String>,
    messages: Vec<WireMessage>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ResponsesPayload {
    pub(super) instructions: Option<String>,
    pub(super) input: Vec<Value>,
}

fn normalize_messages(messages: &[MessageRecord]) -> NormalizedConversation {
    let mut system_parts = Vec::new();
    let mut normalized: Vec<WireMessage> = Vec::new();

    for message in messages {
        if message.content.trim().is_empty() {
            continue;
        }
        let role = match message.role.as_str() {
            "system" | "developer" => {
                system_parts.push(message.content.clone());
                continue;
            }
            "user" => "user",
            "assistant" => "assistant",
            _ => continue,
        };

        if let Some(previous) = normalized.last_mut().filter(|item| item.role == role) {
            previous.content.push_str("\n\n");
            previous.content.push_str(&message.content);
        } else {
            normalized.push(WireMessage {
                role,
                content: message.content.clone(),
            });
        }
    }

    if normalized
        .first()
        .is_some_and(|item| item.role == "assistant")
    {
        normalized.insert(
            0,
            WireMessage {
                role: "user",
                content: OMITTED_USER_CONTEXT.to_owned(),
            },
        );
    }

    let instructions = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };
    NormalizedConversation {
        instructions,
        messages: normalized,
    }
}

pub(super) fn chat_messages(messages: &[MessageRecord]) -> Vec<Value> {
    let conversation = normalize_messages(messages);
    let mut output = Vec::with_capacity(
        conversation.messages.len() + usize::from(conversation.instructions.is_some()),
    );
    if let Some(instructions) = conversation.instructions {
        output.push(json!({"role": "system", "content": instructions}));
    }
    output.extend(
        conversation
            .messages
            .into_iter()
            .map(|message| json!({"role": message.role, "content": message.content})),
    );
    output
}

pub(super) fn responses_payload(
    messages: &[MessageRecord],
    default_instructions: Option<&str>,
) -> ResponsesPayload {
    let conversation = normalize_messages(messages);
    let instructions = conversation.instructions.or_else(|| {
        default_instructions
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    });
    let input = conversation
        .messages
        .into_iter()
        .map(|message| {
            let content_type = if message.role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            json!({
                "type": "message",
                "role": message.role,
                "content": [{"type": content_type, "text": message.content}],
            })
        })
        .collect();
    ResponsesPayload {
        instructions,
        input,
    }
}

pub(super) fn antigravity_body(
    project: &str,
    model: &str,
    messages: &[MessageRecord],
    request_id: &str,
) -> Value {
    let conversation = normalize_messages(messages);
    let contents = conversation
        .messages
        .into_iter()
        .map(|message| {
            json!({
                "role": if message.role == "assistant" { "model" } else { "user" },
                "parts": [{"text": message.content}],
            })
        })
        .collect::<Vec<_>>();
    let mut request = json!({"contents": contents});
    if let Some(instructions) = conversation.instructions {
        request["systemInstruction"] = json!({"parts": [{"text": instructions}]});
    }
    json!({
        "project": project,
        "model": model,
        "request": request,
        "requestType": "agent",
        "userAgent": "antigravity",
        "requestId": request_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, content: &str) -> MessageRecord {
        MessageRecord {
            role: role.to_owned(),
            content: content.to_owned(),
            created_at: "now".into(),
        }
    }

    #[test]
    fn normalization_filters_invalid_messages_and_merges_adjacent_roles() {
        let messages = vec![
            message("system", "first instruction"),
            message("user", "one"),
            message("user", "two"),
            message("assistant", "answer"),
            message("tool", "not valid without a tool-call envelope"),
            message("assistant", "continued"),
            message("user", "  "),
            message("developer", "second instruction"),
        ];
        let normalized = normalize_messages(&messages);
        assert_eq!(
            normalized.instructions.as_deref(),
            Some("first instruction\n\nsecond instruction")
        );
        assert_eq!(normalized.messages.len(), 2);
        assert_eq!(normalized.messages[0].role, "user");
        assert_eq!(normalized.messages[0].content, "one\n\ntwo");
        assert_eq!(normalized.messages[1].role, "assistant");
        assert_eq!(normalized.messages[1].content, "answer\n\ncontinued");
    }

    #[test]
    fn normalization_repairs_a_truncated_history_starting_with_assistant() {
        let normalized = normalize_messages(&[message("assistant", "prior answer")]);
        assert_eq!(normalized.messages.len(), 2);
        assert_eq!(normalized.messages[0].role, "user");
        assert_eq!(normalized.messages[0].content, OMITTED_USER_CONTEXT);
        assert_eq!(normalized.messages[1].role, "assistant");
    }

    #[test]
    fn responses_payload_uses_role_specific_content_types() {
        let payload = responses_payload(
            &[
                message("system", "Be concise"),
                message("user", "question"),
                message("assistant", "answer"),
                message("user", "follow-up"),
            ],
            Some("fallback"),
        );
        assert_eq!(payload.instructions.as_deref(), Some("Be concise"));
        assert_eq!(payload.input[0]["type"], "message");
        assert_eq!(payload.input[0]["content"][0]["type"], "input_text");
        assert_eq!(payload.input[1]["content"][0]["type"], "output_text");
        assert_eq!(payload.input[2]["content"][0]["type"], "input_text");
    }

    #[test]
    fn antigravity_payload_matches_the_cloud_code_assist_envelope() {
        let body = antigravity_body(
            "project-a",
            "gemini-pro-agent",
            &[message("user", "one"), message("user", "two")],
            "agent-test",
        );
        assert_eq!(body["project"], "project-a");
        assert_eq!(body["requestType"], "agent");
        assert_eq!(body["userAgent"], "antigravity");
        assert_eq!(body["requestId"], "agent-test");
        assert_eq!(body["request"]["contents"].as_array().unwrap().len(), 1);
        assert_eq!(
            body["request"]["contents"][0]["parts"][0]["text"],
            "one\n\ntwo"
        );
    }
}
