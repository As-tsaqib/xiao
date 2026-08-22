use std::time::Duration;

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::types::{ApiEnvelope, BotIdentity, SentMessage, Update};

#[derive(Clone)]
pub struct TelegramClient {
    client: Client,
    base: String,
    token: String,
}

impl TelegramClient {
    pub fn new(token: String) -> Result<Self> {
        Ok(Self {
            client: Client::builder().timeout(Duration::from_secs(70)).build()?,
            base: "https://api.telegram.org".into(),
            token,
        })
    }
    #[cfg(test)]
    pub fn with_base(token: String, base: String) -> Result<Self> {
        Ok(Self {
            client: Client::new(),
            base,
            token,
        })
    }
    fn url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.base, self.token, method)
    }

    async fn call<T: DeserializeOwned>(&self, method: &str, body: Value) -> Result<T> {
        for attempt in 0..2 {
            let response = self
                .client
                .post(self.url(method))
                .json(&body)
                .send()
                .await
                .map_err(|e| self.safe_error("transport", e))?;
            let status = response.status();
            let env: ApiEnvelope<T> = response
                .json()
                .await
                .map_err(|e| self.safe_error("decode", e))?;
            if env.ok {
                return env
                    .result
                    .ok_or_else(|| anyhow!("Telegram {method} returned no result"));
            }
            let retry = env.parameters.as_ref().and_then(|p| p.retry_after);
            if attempt == 0 {
                if let Some(seconds) = retry {
                    tokio::time::sleep(Duration::from_secs(seconds.min(30))).await;
                    continue;
                }
            }
            let suffix = retry
                .map(|s| format!("; retry_after={s}s"))
                .unwrap_or_default();
            return Err(anyhow!(
                "Telegram {method} failed ({status}): {}{}",
                env.description.unwrap_or_else(|| "unknown error".into()),
                suffix
            ));
        }
        Err(anyhow!("Telegram {method} failed after retry"))
    }

    fn safe_error(&self, stage: &str, e: reqwest::Error) -> anyhow::Error {
        anyhow!(
            "Telegram {stage} error: {}",
            e.to_string().replace(&self.token, "<redacted>")
        )
    }

    pub async fn get_me(&self) -> Result<BotIdentity> {
        self.call("getMe", json!({})).await
    }
    pub async fn get_updates(&self, offset: Option<i64>, timeout: u64) -> Result<Vec<Update>> {
        self.call(
            "getUpdates",
            with_optional(
                json!({"timeout":timeout,"allowed_updates":["message","callback_query"]}),
                "offset",
                offset.map(Value::from),
            ),
        )
        .await
    }
    pub async fn send_rich(
        &self,
        chat_id: i64,
        rich: Value,
        markup: Option<Value>,
    ) -> Result<SentMessage> {
        self.call(
            "sendRichMessage",
            with_optional(
                json!({"chat_id":chat_id,"rich_message":rich}),
                "reply_markup",
                markup,
            ),
        )
        .await
    }
    pub async fn send_plain(
        &self,
        chat_id: i64,
        text: &str,
        markup: Option<Value>,
    ) -> Result<SentMessage> {
        self.call(
            "sendMessage",
            with_optional(
                json!({"chat_id":chat_id,"text":text}),
                "reply_markup",
                markup,
            ),
        )
        .await
    }
    pub async fn draft_rich(&self, chat_id: i64, draft_id: i64, rich: Value) -> Result<bool> {
        self.call("sendRichMessageDraft", json!({"chat_id":chat_id,"draft_id":if draft_id==0{1}else{draft_id},"rich_message":rich})).await
    }
    pub async fn edit_rich(
        &self,
        chat_id: i64,
        message_id: i64,
        rich: Value,
        markup: Option<Value>,
    ) -> Result<SentMessage> {
        self.call(
            "editMessageText",
            with_optional(
                json!({"chat_id":chat_id,"message_id":message_id,"rich_message":rich}),
                "reply_markup",
                markup,
            ),
        )
        .await
    }
    pub async fn edit_plain(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        markup: Option<Value>,
    ) -> Result<SentMessage> {
        self.call(
            "editMessageText",
            with_optional(
                json!({"chat_id":chat_id,"message_id":message_id,"text":text}),
                "reply_markup",
                markup,
            ),
        )
        .await
    }
    pub async fn edit_markup(
        &self,
        chat_id: i64,
        message_id: i64,
        markup: Option<Value>,
    ) -> Result<SentMessage> {
        self.call(
            "editMessageReplyMarkup",
            with_optional(
                json!({"chat_id":chat_id,"message_id":message_id}),
                "reply_markup",
                markup,
            ),
        )
        .await
    }
    pub async fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<bool> {
        self.call(
            "deleteMessage",
            json!({"chat_id":chat_id,"message_id":message_id}),
        )
        .await
    }
    pub async fn answer_callback(
        &self,
        id: &str,
        text: Option<&str>,
        show_alert: bool,
    ) -> Result<bool> {
        self.call(
            "answerCallbackQuery",
            with_optional(
                json!({"callback_query_id":id,"show_alert":show_alert}),
                "text",
                text.map(Value::from),
            ),
        )
        .await
    }
}

fn with_optional(mut body: Value, key: &str, value: Option<Value>) -> Value {
    if let Some(value) = value {
        body.as_object_mut()
            .expect("Telegram request body must be an object")
            .insert(key.to_owned(), value);
    }
    body
}

#[async_trait::async_trait]
impl super::menu::EditTransport for TelegramClient {
    async fn edit_rich_surface(
        &self,
        chat_id: i64,
        message_id: i64,
        rich: Value,
        markup: Value,
    ) -> Result<()> {
        self.edit_rich(chat_id, message_id, rich, Some(markup))
            .await
            .map(|_| ())
    }
    async fn edit_plain_surface(
        &self,
        chat_id: i64,
        message_id: i64,
        text: String,
        markup: Value,
    ) -> Result<()> {
        self.edit_plain(chat_id, message_id, &text, Some(markup))
            .await
            .map(|_| ())
    }
    async fn send_rich_replacement(&self, chat_id: i64, rich: Value, markup: Value) -> Result<i64> {
        Ok(self
            .send_rich(chat_id, rich, Some(markup))
            .await?
            .message_id)
    }
    async fn send_plain_replacement(
        &self,
        chat_id: i64,
        text: String,
        markup: Value,
    ) -> Result<i64> {
        Ok(self
            .send_plain(chat_id, &text, Some(markup))
            .await?
            .message_id)
    }
    async fn retire_keyboard(&self, chat_id: i64, message_id: i64) -> Result<()> {
        self.edit_markup(chat_id, message_id, Some(json!({"inline_keyboard":[]})))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_optional_fields_are_omitted_not_serialized_as_null() {
        let body = with_optional(json!({"chat_id": 7, "text": "ok"}), "reply_markup", None);
        assert_eq!(body, json!({"chat_id": 7, "text": "ok"}));
        assert!(!body.as_object().unwrap().contains_key("reply_markup"));
    }

    #[test]
    fn present_optional_fields_are_preserved() {
        let markup = json!({"inline_keyboard": []});
        let body = with_optional(
            json!({"chat_id": 7, "text": "ok"}),
            "reply_markup",
            Some(markup.clone()),
        );
        assert_eq!(body.get("reply_markup"), Some(&markup));
    }
}
