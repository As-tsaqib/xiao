use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
    pub callback_query: Option<CallbackQuery>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Message {
    pub message_id: i64,
    #[serde(default)]
    pub message_thread_id: Option<i64>,
    pub chat: Chat,
    pub from: Option<User>,
    pub text: Option<String>,
}

impl Message {
    pub fn scope(&self) -> super::scope::TelegramScope {
        super::scope::TelegramScope::new(self.chat.id, self.message_thread_id)
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct User {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub username: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    pub message: Option<Message>,
    pub data: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SentMessage {
    pub message_id: i64,
    pub chat: Chat,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BotIdentity {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiEnvelope<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
    pub parameters: Option<ResponseParameters>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct ResponseParameters {
    pub retry_after: Option<u64>,
}
