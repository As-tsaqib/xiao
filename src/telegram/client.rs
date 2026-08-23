use std::time::Duration;

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::path::Path;

use super::commands::BotCommand;
use super::types::{ApiEnvelope, BotIdentity, SentMessage, Update};
use super::TelegramScope;

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
                self.safe_description(&env.description.unwrap_or_else(|| "unknown error".into())),
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

    fn safe_description(&self, value: &str) -> String {
        crate::security::redact::redact_text(&value.replace(&self.token, "<redacted>"))
    }

    pub async fn get_me(&self) -> Result<BotIdentity> {
        self.call("getMe", json!({})).await
    }
    pub async fn set_my_commands(&self, commands: &[BotCommand]) -> Result<bool> {
        self.call("setMyCommands", json!({"commands":commands}))
            .await
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
        self.send_rich_scoped(TelegramScope::new(chat_id, None), rich, markup)
            .await
    }
    pub async fn send_rich_scoped(
        &self,
        scope: TelegramScope,
        rich: Value,
        markup: Option<Value>,
    ) -> Result<SentMessage> {
        let body = with_optional(
            json!({"chat_id":scope.chat_id,"rich_message":rich}),
            "message_thread_id",
            scope.message_thread_id.map(Value::from),
        );
        self.call(
            "sendRichMessage",
            with_optional(body, "reply_markup", markup),
        )
        .await
    }
    pub async fn send_plain(
        &self,
        chat_id: i64,
        text: &str,
        markup: Option<Value>,
    ) -> Result<SentMessage> {
        self.send_plain_scoped(TelegramScope::new(chat_id, None), text, markup)
            .await
    }
    pub async fn send_plain_scoped(
        &self,
        scope: TelegramScope,
        text: &str,
        markup: Option<Value>,
    ) -> Result<SentMessage> {
        let body = with_optional(
            json!({"chat_id":scope.chat_id,"text":text}),
            "message_thread_id",
            scope.message_thread_id.map(Value::from),
        );
        self.call("sendMessage", with_optional(body, "reply_markup", markup))
            .await
    }
    pub async fn send_document(
        &self,
        chat_id: i64,
        path: &Path,
        filename: &str,
    ) -> Result<SentMessage> {
        self.send_document_scoped(TelegramScope::new(chat_id, None), path, filename)
            .await
    }
    pub async fn send_document_scoped(
        &self,
        scope: TelegramScope,
        path: &Path,
        filename: &str,
    ) -> Result<SentMessage> {
        let metadata = tokio::fs::metadata(path).await?;
        if !metadata.is_file() || metadata.len() > 50 * 1024 * 1024 {
            return Err(anyhow!("Telegram result file is missing or exceeds 50 MiB"));
        }
        let bytes = tokio::fs::read(path).await?;
        let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.to_owned());
        let mut form = reqwest::multipart::Form::new().text("chat_id", scope.chat_id.to_string());
        if let Some(thread) = scope.message_thread_id {
            form = form.text("message_thread_id", thread.to_string());
        }
        let form = form.part("document", part);
        let response = self
            .client
            .post(self.url("sendDocument"))
            .multipart(form)
            .send()
            .await
            .map_err(|error| self.safe_error("document transport", error))?;
        let status = response.status();
        let envelope: ApiEnvelope<SentMessage> = response
            .json()
            .await
            .map_err(|error| self.safe_error("document decode", error))?;
        if envelope.ok {
            envelope
                .result
                .ok_or_else(|| anyhow!("Telegram sendDocument returned no result"))
        } else {
            Err(anyhow!(
                "Telegram sendDocument failed ({status}): {}",
                self.safe_description(
                    &envelope
                        .description
                        .unwrap_or_else(|| "unknown error".into())
                )
            ))
        }
    }
    pub async fn draft_rich(&self, chat_id: i64, draft_id: i64, rich: Value) -> Result<bool> {
        self.draft_rich_scoped(TelegramScope::new(chat_id, None), draft_id, rich)
            .await
    }
    pub async fn draft_rich_scoped(
        &self,
        scope: TelegramScope,
        draft_id: i64,
        rich: Value,
    ) -> Result<bool> {
        let body = with_optional(
            json!({"chat_id":scope.chat_id,"draft_id":if draft_id==0{1}else{draft_id},"rich_message":rich}),
            "message_thread_id",
            scope.message_thread_id.map(Value::from),
        );
        self.call("sendRichMessageDraft", body).await
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
    async fn send_rich_replacement(
        &self,
        scope: TelegramScope,
        rich: Value,
        markup: Value,
    ) -> Result<i64> {
        Ok(self
            .send_rich_scoped(scope, rich, Some(markup))
            .await?
            .message_id)
    }
    async fn send_plain_replacement(
        &self,
        scope: TelegramScope,
        text: String,
        markup: Value,
    ) -> Result<i64> {
        Ok(self
            .send_plain_scoped(scope, &text, Some(markup))
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
    use axum::{
        body::Bytes,
        extract::State,
        http::{HeaderMap, Uri},
        routing::post,
        Json, Router,
    };
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    #[derive(Default)]
    struct UploadProbe {
        path: Mutex<String>,
        content_type: Mutex<String>,
        body: Mutex<Vec<u8>>,
    }

    #[derive(Default)]
    struct RequestProbe {
        requests: Mutex<Vec<(String, String, Vec<u8>)>>,
    }

    async fn request_stub(
        State(probe): State<Arc<RequestProbe>>,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Json<Value> {
        let method = uri.path().rsplit('/').next().unwrap_or_default();
        probe.requests.lock().unwrap().push((
            method.to_owned(),
            headers
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
            body.to_vec(),
        ));
        let result = if matches!(method, "setMyCommands" | "sendRichMessageDraft") {
            json!(true)
        } else {
            json!({"message_id":91,"chat":{"id":4242,"type":"private"}})
        };
        Json(json!({"ok":true,"result":result}))
    }

    async fn upload_stub(
        State(probe): State<Arc<UploadProbe>>,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Json<Value> {
        *probe.path.lock().unwrap() = uri.path().to_owned();
        *probe.content_type.lock().unwrap() = headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        *probe.body.lock().unwrap() = body.to_vec();
        Json(json!({
            "ok":true,
            "result":{"message_id":91,"chat":{"id":4242,"type":"private"}}
        }))
    }

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

    #[tokio::test]
    async fn result_file_is_sent_through_telegram_multipart_document_path() {
        let probe = Arc::new(UploadProbe::default());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .fallback(post(upload_stub))
            .with_state(probe.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("result.txt");
        std::fs::write(&artifact, "observable artifact content").unwrap();
        let client =
            TelegramClient::with_base("test-token".into(), format!("http://{address}")).unwrap();
        let sent = client
            .send_document(4242, &artifact, "result.txt")
            .await
            .unwrap();
        assert_eq!(sent.message_id, 91);
        assert_eq!(&*probe.path.lock().unwrap(), "/bottest-token/sendDocument");
        assert!(probe
            .content_type
            .lock()
            .unwrap()
            .starts_with("multipart/form-data; boundary="));
        let body = String::from_utf8_lossy(&probe.body.lock().unwrap()).into_owned();
        assert!(body.contains("name=\"chat_id\""));
        assert!(body.contains("4242"));
        assert!(body.contains("filename=\"result.txt\""));
        assert!(body.contains("observable artifact content"));
    }

    #[tokio::test]
    async fn every_outbound_creation_path_preserves_the_topic_scope() {
        let probe = Arc::new(RequestProbe::default());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .fallback(post(request_stub))
            .with_state(probe.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client =
            TelegramClient::with_base("test-token".into(), format!("http://{address}")).unwrap();
        let scope = TelegramScope::new(4242, Some(73));
        client
            .send_rich_scoped(scope, json!({"blocks":[]}), None)
            .await
            .unwrap();
        client
            .send_plain_scoped(scope, "topic reply", None)
            .await
            .unwrap();
        client
            .draft_rich_scoped(scope, 8, json!({"blocks":[]}))
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("topic.txt");
        std::fs::write(&artifact, "topic artifact").unwrap();
        client
            .send_document_scoped(scope, &artifact, "topic.txt")
            .await
            .unwrap();

        let requests = probe.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        for (method, content_type, body) in requests.iter() {
            if method == "sendDocument" {
                assert!(content_type.starts_with("multipart/form-data"));
                let body = String::from_utf8_lossy(body);
                assert!(body.contains("name=\"message_thread_id\""));
                assert!(body.contains("73"));
            } else {
                let body: Value = serde_json::from_slice(body).unwrap();
                assert_eq!(body["message_thread_id"], 73);
            }
        }
        server.abort();
    }

    #[tokio::test]
    async fn set_my_commands_payload_comes_from_the_public_registry() {
        let probe = Arc::new(RequestProbe::default());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .fallback(post(request_stub))
            .with_state(probe.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client =
            TelegramClient::with_base("test-token".into(), format!("http://{address}")).unwrap();
        let expected = super::super::commands::TelegramCommandRegistry::bot_commands();
        assert!(client.set_my_commands(&expected).await.unwrap());
        let requests = probe.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "setMyCommands");
        let body: Value = serde_json::from_slice(&requests[0].2).unwrap();
        assert_eq!(body["commands"], serde_json::to_value(expected).unwrap());
        server.abort();
    }
}
