pub mod acl;
pub mod client;
pub mod commands;
mod login;
pub mod menu;
pub mod paginator;
pub mod rich;
pub mod scope;
pub mod types;

pub use scope::TelegramScope;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{anyhow, Result};
use serde_json::json;
use tokio::{
    sync::{mpsc, Mutex as AsyncMutex},
    time::{interval, sleep, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;

use crate::{
    agent::AgentAnswer,
    app::AppState,
    attachments::{AttachmentIngest, AttachmentKind},
    command::CommandResult,
    presentation::{
        Action, ActionTarget, Block, ProgressActivity, ProgressIcon, ProgressItem, ProgressState,
        View,
    },
    providers::AgentEvent,
    security::secrets::SecretStore,
};

use acl::AccessPolicy;
use client::TelegramClient;
use commands::TelegramCommandRegistry;
use login::{CustomLoginPhase, CustomLoginStore};
use menu::{action_at, keyboard, parse_callback, MenuSession, MenuStore};
use types::{CallbackQuery, Message, Update};

#[derive(Clone)]
pub struct TelegramAdapter {
    app: AppState,
    client: TelegramClient,
    menus: Arc<MenuStore>,
    custom_logins: Arc<CustomLoginStore>,
    /// Non-generation semantic updates for one Telegram principal are serialized so
    /// two callbacks/commands cannot race a multi-step UI/state transition. Long
    /// generation deliberately does not hold this lane; `/stop` remains a fast path.
    principal_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
    /// One parent cancellation token per in-flight Telegram message work item.
    /// It spans bot download, attachment processing and the resulting agent
    /// run; the keyed list tolerates an already-accepted concurrent update.
    active_work: Arc<Mutex<HashMap<String, Vec<CancellationToken>>>>,
}

struct CustomInputContext<'a> {
    scope: TelegramScope,
    user_id: i64,
    update_id: i64,
    message_id: i64,
    principal: &'a str,
}

impl TelegramAdapter {
    pub async fn from_app(app: AppState) -> Result<Self> {
        let cfg = app.config.read().await.clone();
        let secrets = SecretStore::new(cfg.paths.secrets_dir.clone());
        let control = app.storage.telegram_control_state()?;
        let token = control
            .as_ref()
            .and_then(|state| state.bot_token_ref.as_deref())
            .map(|reference| secrets.get(reference))
            .transpose()?
            .flatten()
            .ok_or_else(|| anyhow!("Telegram is enabled but bot token is not configured"))?;
        let client = TelegramClient::new(token)?;
        if let Ok(bot) = client.get_me().await {
            let _ = app
                .storage
                .set_telegram_bot_identity(&serde_json::to_string(&bot)?);
        }
        client
            .set_my_commands(&TelegramCommandRegistry::bot_commands())
            .await?;
        Ok(Self {
            app,
            client,
            menus: Arc::new(MenuStore::new(Duration::from_secs(
                cfg.telegram.ui.menu_ttl_seconds,
            ))),
            custom_logins: Arc::new(CustomLoginStore::new(Duration::from_secs(
                cfg.telegram.ui.menu_ttl_seconds,
            ))),
            principal_locks: Arc::new(Mutex::new(HashMap::new())),
            active_work: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn run(self) -> Result<()> {
        let interrupted = self.app.storage.quarantine_telegram_processing()?;
        if interrupted > 0 {
            tracing::warn!(interrupted, "quarantined Telegram updates interrupted during processing; automatic replay is intentionally disabled to avoid duplicating side effects");
        }
        let mut offset = self
            .app
            .storage
            .telegram_state("offset")?
            .and_then(|s| s.parse::<i64>().ok());
        let mut backoff = 1u64;
        // Resume updates accepted before a crash but not yet marked processed.
        for record in self.app.storage.pending_telegram_updates(500)? {
            match serde_json::from_str::<Update>(&record.payload_json) {
                Ok(update) => self.spawn_update(update),
                Err(error) => {
                    let _ = self
                        .app
                        .storage
                        .mark_telegram_failed(record.update_id, &format!("decode: {error}"));
                }
            }
        }
        loop {
            // SQLite control state is authoritative. The in-memory TOML is a
            // compatibility projection and may lag after a failed snapshot.
            let enabled = self
                .app
                .storage
                .telegram_control_state()?
                .is_some_and(|state| state.enabled);
            if !enabled {
                self.app.health.set_telegram_polling(false).await;
                sleep(Duration::from_secs(2)).await;
                continue;
            }
            match self.client.get_updates(offset, 50).await {
                Ok(updates) => {
                    self.app.health.set_telegram_polling(true).await;
                    backoff = 1;
                    for update in updates {
                        self.app.health.mark_telegram_update().await;
                        let payload = serde_json::to_string(&update)?;
                        let accepted = self
                            .app
                            .storage
                            .enqueue_telegram_update(update.update_id, &payload)?;
                        offset = Some(update.update_id + 1);
                        if accepted {
                            self.spawn_update(update);
                        }
                    }
                }
                Err(error) => {
                    self.app.health.set_telegram_polling(false).await;
                    tracing::warn!(%error, "Telegram long polling failed");
                    sleep(Duration::from_secs(backoff.min(30))).await;
                    backoff = (backoff * 2).min(30);
                }
            }
        }
    }

    fn spawn_update(&self, update: Update) {
        let adapter = self.clone();
        tokio::spawn(async move {
            let id = update.update_id;
            match adapter.app.storage.mark_telegram_processing(id) {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => {
                    tracing::warn!(%error, update_id=id, "failed to claim Telegram inbox update");
                    return;
                }
            }
            match adapter.handle_update(update).await {
                Ok(()) => {
                    if let Err(error) = adapter.app.storage.mark_telegram_processed(id) {
                        tracing::warn!(%error,update_id=id,"failed to mark Telegram update processed");
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, update_id=id, "Telegram update handling failed");
                    let safe = crate::security::redact::redact_text(&error.to_string());
                    let _ = adapter.app.storage.mark_telegram_failed(id, &safe);
                }
            }
        });
    }

    async fn policy(&self) -> AccessPolicy {
        if let Ok(Some(control)) = self.app.storage.telegram_control_state() {
            return AccessPolicy {
                allowed_chat_ids: control.allowed_chat_ids,
                owner_user_id: control.owner_user_id,
                owner_resolution_required: self
                    .app
                    .storage
                    .owner_resolution_candidates()
                    .map(|candidates| !candidates.is_empty())
                    .unwrap_or(true),
            };
        }
        AccessPolicy {
            allowed_chat_ids: Vec::new(),
            owner_user_id: None,
            owner_resolution_required: true,
        }
    }
    async fn allowed(&self, chat_id: i64, user_id: Option<i64>, kind: &str) -> bool {
        self.policy().await.allows(chat_id, user_id, kind)
    }
    #[cfg(test)]
    fn principal(app: &AppState, user_id: i64) -> String {
        app.storage
            .management_owner_id()
            .unwrap_or_else(|_| app.resolve_telegram_owner(user_id).unwrap().owner_id)
    }
    fn principal_lock(&self, principal: &str) -> Arc<AsyncMutex<()>> {
        let mut lanes = self
            .principal_locks
            .lock()
            .expect("Telegram principal lane mutex poisoned");
        lanes
            .entry(principal.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    fn work_key(principal: &str, scope: TelegramScope) -> String {
        format!("{principal}:{}:{}", scope.chat_id, scope.thread_key())
    }

    fn begin_work(&self, principal: &str, scope: TelegramScope) -> CancellationToken {
        let token = CancellationToken::new();
        if let Ok(mut active) = self.active_work.lock() {
            active
                .entry(Self::work_key(principal, scope))
                .or_default()
                .push(token.clone());
        }
        token
    }

    fn finish_work(&self, principal: &str, scope: TelegramScope, token: &CancellationToken) {
        if let Ok(mut active) = self.active_work.lock() {
            let key = Self::work_key(principal, scope);
            if let Some(tokens) = active.get_mut(&key) {
                if let Some(index) = tokens.iter().position(|registered| registered == token) {
                    tokens.remove(index);
                }
                if tokens.is_empty() {
                    active.remove(&key);
                }
            }
        }
    }

    fn cancel_work(&self, principal: &str, scope: TelegramScope) -> bool {
        self.active_work
            .lock()
            .ok()
            .and_then(|active| active.get(&Self::work_key(principal, scope)).cloned())
            .map(|tokens| {
                for token in tokens {
                    token.cancel();
                }
                true
            })
            .unwrap_or(false)
    }
    async fn execute_serialized(
        &self,
        principal: &str,
        scope: TelegramScope,
        input: &str,
    ) -> Result<CommandResult> {
        let lane = self.principal_lock(principal);
        let _guard = lane.lock().await;
        self.app
            .commands
            .execute_text_in_telegram_scope(principal, scope, input, None)
            .await
    }

    async fn execute_internal_serialized(
        &self,
        principal: &str,
        scope: TelegramScope,
        input: &str,
    ) -> Result<CommandResult> {
        let lane = self.principal_lock(principal);
        let _guard = lane.lock().await;
        self.app
            .commands
            .execute_callback_in_telegram_scope(principal, scope, input, None)
            .await
    }

    async fn handle_update(&self, update: Update) -> Result<()> {
        self.cleanup_expired_custom_credentials();
        if let Some(callback) = update.callback_query {
            return self.handle_callback(callback).await;
        }
        if let Some(message) = update.message {
            return self.handle_message(update.update_id, message).await;
        }
        Ok(())
    }

    fn cleanup_expired_custom_credentials(&self) {
        for reference in self.custom_logins.take_expired_credential_refs() {
            let _ = self.app.auth.logout(&reference);
        }
    }

    async fn handle_message(&self, update_id: i64, message: Message) -> Result<()> {
        let Some(user) = message.from.as_ref() else {
            return Ok(());
        };
        if !self
            .allowed(message.chat.id, Some(user.id), &message.chat.kind)
            .await
        {
            return Ok(());
        }
        let scope = message.scope();
        let principal = self.app.resolve_telegram_owner(user.id)?.owner_id;

        // `/stop` is deliberately evaluated before pending rename/login input
        // and before the principal UI lane.  A running task must remain
        // cancellable even while a scoped menu is waiting for text.
        if message.text.as_deref().is_some_and(is_stop_command) {
            self.cancel_work(&principal, scope);
            return match self
                .app
                .commands
                .execute_text_in_telegram_scope(
                    &principal,
                    scope,
                    message.text.as_deref().unwrap_or_default(),
                    None,
                )
                .await
            {
                Ok(result) => self.send_result(scope, user.id, result).await,
                Err(error) => self
                    .send_view(
                        scope,
                        &View::info(
                            "ERROR",
                            crate::security::redact::redact_text(&error.to_string()),
                        ),
                        None,
                    )
                    .await
                    .map(|_| ()),
            };
        }

        // UI capture is checked after ACL but before slash parsing or agent dispatch.
        if let Some(menu) = self.menus.pending_for_scope(scope, user.id) {
            let mut guard = menu.lock().await;
            if let Some(text) = message.text.as_deref() {
                if let Some(prefix) = guard.pending_input.take() {
                    if prefix.starts_with("custom:") {
                        if let Err(error) = self
                            .handle_custom_input(
                                &mut guard,
                                &prefix,
                                text,
                                CustomInputContext {
                                    scope,
                                    user_id: user.id,
                                    update_id,
                                    message_id: message.message_id,
                                    principal: &principal,
                                },
                            )
                            .await
                        {
                            let wizard_id = prefix.split(':').nth(1).unwrap_or_default();
                            let endpoint = self
                                .custom_logins
                                .get(wizard_id)
                                .and_then(|wizard| wizard.try_lock().ok()?.endpoint.clone());
                            guard.current_view = login::failure_view(
                                wizard_id,
                                endpoint.as_deref(),
                                &classify_custom_error(&error),
                            );
                        }
                        guard.revision += 1;
                        self.advance_menu_prompt(&mut guard).await?;
                        return Ok(());
                    }
                    let command = format!("{} {}", prefix, text.trim());
                    match self
                        .execute_internal_serialized(&principal, scope, &command)
                        .await
                    {
                        Ok(result) => {
                            let next = result_view(result)?;
                            guard.current_view = next;
                            guard.revision += 1;
                            self.edit_first(&mut guard).await?;
                            return Ok(());
                        }
                        Err(error) => {
                            guard.pending_input = Some(prefix);
                            guard.current_view = View {
                                title: Some("RENAME SESSION".into()),
                                blocks: vec![Block::Paragraph {
                                    text: format!("{}\nSend another name or use Back.", error),
                                }],
                                actions: vec![vec![Action::back(), Action::close()]],
                                side_mode: false,
                            };
                            guard.revision += 1;
                            self.edit_first(&mut guard).await?;
                            return Ok(());
                        }
                    }
                }
            }
        }

        let work = self.begin_work(&principal, scope);
        let result = async {
            let attachment_prompt = match self
                .ingest_telegram_attachment(&principal, scope, &message, &work)
                .await
            {
                Ok(prompt) => prompt,
                Err(error) => {
                    self.send_view(
                        scope,
                        &View::info(
                            "ATTACHMENT ERROR",
                            crate::security::redact::redact_text(&error.to_string()),
                        ),
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };
            let text = match attachment_prompt.as_deref().or(message.text.as_deref()) {
                Some(text) => text,
                None => return Ok(()),
            };

            let is_agent_request =
                !text.trim_start().starts_with('/') || text.trim_start().starts_with("/retry");
            let result = if is_agent_request {
                // Every Telegram generation receives observable live events so an
                // exact ASK decision can surface as a scoped inline card. Private
                // chats retain the existing draft transport; topics intentionally
                // receive no draft updates but do receive the same approval card.
                self.execute_with_live_events(
                    &principal,
                    scope,
                    update_id,
                    user.id,
                    text,
                    (message.chat.kind == "private").then_some(update_id),
                    work.child_token(),
                )
                .await
            } else {
                self.execute_serialized(&principal, scope, text).await
            };
            match result {
                Ok(value) => self.send_result(scope, user.id, value).await,
                Err(error) => self
                    .send_view(scope, &View::info("ERROR", error.to_string()), None)
                    .await
                    .map(|_| ()),
            }
        }
        .await;
        self.finish_work(&principal, scope, &work);
        result
    }

    async fn ingest_telegram_attachment(
        &self,
        principal: &str,
        scope: TelegramScope,
        message: &Message,
        cancellation: &CancellationToken,
    ) -> Result<Option<String>> {
        let selected = if let Some(photo) = message
            .photo
            .iter()
            .max_by_key(|photo| u64::from(photo.width).saturating_mul(u64::from(photo.height)))
        {
            Some((
                AttachmentKind::Image,
                photo.file_id.as_str(),
                photo.file_unique_id.as_str(),
                format!("photo-{}.jpg", message.message_id),
                None,
                photo.file_size,
            ))
        } else {
            message.document.as_ref().map(|document| {
                (
                    AttachmentKind::Document,
                    document.file_id.as_str(),
                    document.file_unique_id.as_str(),
                    document
                        .file_name
                        .clone()
                        .unwrap_or_else(|| format!("document-{}", message.message_id)),
                    document.mime_type.clone(),
                    document.file_size,
                )
            })
        };
        let Some((kind, file_id, unique_id, original_name, declared_mime, declared_size)) =
            selected
        else {
            return Ok(None);
        };
        let max_bytes = self.app.attachments.max_download_bytes(kind);
        if declared_size.is_some_and(|size| size > max_bytes) {
            return Err(anyhow!(
                "Telegram attachment exceeds the configured {} byte limit",
                max_bytes
            ));
        }
        let file = self
            .client
            .get_file_with_cancellation(file_id, cancellation)
            .await?;
        if file.file_size.is_some_and(|size| size > max_bytes) {
            return Err(anyhow!(
                "Telegram attachment exceeds the configured {} byte limit",
                max_bytes
            ));
        }
        let bytes = self
            .client
            .download_file_bounded_with_cancellation(&file.file_path, max_bytes, cancellation)
            .await?;
        let context = self.app.sessions.context_for_telegram(principal, scope)?;
        let manager = self.app.attachments.clone();
        let processing_timeout = manager.processing_timeout();
        let owner = principal.to_owned();
        let session_id = context.active.id;
        let telegram_file_id = file_id.to_owned();
        let telegram_unique_id = unique_id.to_owned();
        let processing_token = cancellation.child_token();
        let mut processing = tokio::task::spawn_blocking(move || {
            manager.ingest_with_cancellation(
                AttachmentIngest {
                    owner_id: owner,
                    session_id,
                    telegram_file_id: Some(telegram_file_id),
                    telegram_unique_id: Some(telegram_unique_id),
                    original_name,
                    declared_mime,
                    expected_kind: kind,
                    bytes,
                },
                processing_token,
            )
        });
        let record = match tokio::time::timeout(processing_timeout, &mut processing).await {
            Ok(result) => {
                result.map_err(|_| anyhow!("attachment processor terminated unexpectedly"))??
            }
            Err(_) => {
                cancellation.cancel();
                let _ = processing.await;
                return Err(anyhow!(
                    "attachment processing exceeded the configured timeout"
                ));
            }
        };
        let caption = message
            .caption
            .as_deref()
            .map(str::trim)
            .filter(|caption| !caption.is_empty())
            .unwrap_or(match kind {
                AttachmentKind::Image => {
                    "Describe this image and answer based only on what is visibly supported."
                }
                AttachmentKind::Document => {
                    "Read this document and summarize the relevant content."
                }
            });
        Ok(Some(format!(
            "Attachment received: {} (id={}, type={}, status={}). {}",
            record.original_name,
            record.attachment_id,
            record.detected_mime,
            record.processing_status,
            caption
        )))
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_with_live_events(
        &self,
        principal: &str,
        scope: TelegramScope,
        update_id: i64,
        user_id: i64,
        text: &str,
        draft_id: Option<i64>,
        cancellation: CancellationToken,
    ) -> Result<CommandResult> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let is_retry = text.trim_start().starts_with("/retry");
        let future = async {
            if is_retry {
                self.app
                    .commands
                    .retry_with_progress_in_telegram_scope_with_cancellation(
                        principal,
                        scope,
                        Some(tx),
                        cancellation.child_token(),
                    )
                    .await
            } else {
                self.app
                    .commands
                    .execute_text_in_telegram_scope_with_cancellation(
                        principal,
                        scope,
                        text,
                        Some(tx),
                        cancellation.child_token(),
                    )
                    .await
            }
        };
        tokio::pin!(future);
        let mut ticker = interval(Duration::from_millis(750));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let configured_detail = self
            .app
            .config
            .read()
            .await
            .telegram
            .ui
            .progress_detail
            .clone();
        let detail = self
            .app
            .storage
            .setting(&format!("telegram.progress_detail:{principal}"))?
            .unwrap_or(configured_detail);
        let mut aggregator = ProgressAggregator::new(detail);
        let mut dirty = true;
        let mut last_sent = std::time::Instant::now() - Duration::from_secs(30);
        const HEARTBEAT: Duration = Duration::from_secs(20);
        loop {
            tokio::select! {
                result = &mut future => {
                    if dirty {
                        if let Some(draft_id) = draft_id {
                            let view = aggregator.view();
                            let _ = self.client.draft_rich_scoped(scope, draft_id, rich::render(&view, true)).await;
                        }
                    }
                    return result;
                }
                event = rx.recv() => {
                    if let Some(event) = event {
                        if let AgentEvent::ApprovalRequested {
                            approval_id,
                            tool,
                            summary,
                            ..
                        } = &event
                        {
                            if let Err(error) = self
                                .send_approval_card(scope, user_id, approval_id, tool, summary)
                                .await
                            {
                                tracing::warn!(%error, update_id, "failed to send Telegram approval card");
                            }
                        }
                        aggregator.push(event);
                        dirty = true;
                    }
                }
                _ = ticker.tick() => {
                    if dirty || last_sent.elapsed() >= HEARTBEAT {
                        if let Some(draft_id) = draft_id {
                            let view = aggregator.view();
                            let _ = self.client.draft_rich_scoped(scope, draft_id, rich::render(&view, true)).await;
                            dirty = false;
                            last_sent = std::time::Instant::now();
                        }
                    }
                }
            }
        }
    }

    /// Emit a one-shot approval card through the normal scoped menu system.
    /// The opaque approval id remains in memory behind callback indirection;
    /// it is never made a public Telegram command or exposed in message text.
    async fn send_approval_card(
        &self,
        scope: TelegramScope,
        user_id: i64,
        approval_id: &str,
        tool: &str,
        summary: &str,
    ) -> Result<()> {
        let safe_tool = safe_progress(tool);
        let safe_summary = safe_progress(summary);
        let view = View {
            title: Some("APPROVAL REQUIRED".into()),
            blocks: vec![Block::Paragraph {
                text: format!(
                    "{safe_tool} is waiting for your one-time decision.\n{safe_summary}\n\nThis approval is bound to this owner, chat/topic, run, tool call, arguments and expiry."
                ),
            }],
            actions: vec![vec![
                Action::command("Approve once", format!("/_approval:approve:{approval_id}")),
                Action::command("Deny", format!("/_approval:deny:{approval_id}")),
            ]],
            side_mode: false,
        };
        self.send_menu(scope, user_id, view).await?;
        Ok(())
    }

    async fn send_view(
        &self,
        scope: TelegramScope,
        view: &View,
        markup: Option<serde_json::Value>,
    ) -> Result<i64> {
        match self
            .client
            .send_rich_scoped(scope, rich::render(view, false), markup.clone())
            .await
        {
            Ok(sent) => Ok(sent.message_id),
            Err(_) => Ok(self
                .client
                .send_plain_scoped(scope, &rich::plain(view), markup)
                .await?
                .message_id),
        }
    }

    async fn send_result(
        &self,
        scope: TelegramScope,
        user_id: i64,
        result: CommandResult,
    ) -> Result<()> {
        match result {
            CommandResult::Agent(answer) => {
                let artifacts = answer.artifacts.clone();
                let view = agent_final_view(answer);
                for page in paginate_final_view(&view, 3500) {
                    self.send_view(scope, &page, None).await?;
                }
                for artifact in artifacts {
                    if self.safe_artifact(&artifact.path) {
                        self.client
                            .send_document_scoped(scope, &artifact.path, &artifact.name)
                            .await?;
                    }
                }
                Ok(())
            }
            CommandResult::StartCustomLogin => {
                self.start_custom_login(scope, user_id).await?;
                Ok(())
            }
            CommandResult::InputRequest {
                view,
                command_prefix,
            } => {
                let menu = self.send_menu(scope, user_id, view).await?;
                menu.lock().await.pending_input = Some(command_prefix);
                Ok(())
            }
            other => {
                let view = result_view(other)?;
                if view.actions.is_empty() {
                    self.send_view(scope, &view, None).await?;
                } else {
                    self.send_menu(scope, user_id, view).await?;
                }
                Ok(())
            }
        }
    }

    async fn start_custom_login(&self, scope: TelegramScope, user_id: i64) -> Result<()> {
        let menu = self
            .send_menu(scope, user_id, login::endpoint_view("pending"))
            .await?;
        let menu_id = menu.lock().await.id.clone();
        let wizard = self.custom_logins.begin(scope, user_id, menu_id);
        let mut wizard_guard = wizard.lock().await;
        wizard_guard.protocol = self
            .app
            .config
            .read()
            .await
            .providers
            .custom
            .protocol
            .clone();
        let wizard_id = wizard_guard.id.clone();
        drop(wizard_guard);
        let mut guard = menu.lock().await;
        guard.current_view = login::endpoint_view(&wizard_id);
        guard.pending_input = Some(format!("custom:{wizard_id}:endpoint"));
        guard.revision += 1;
        self.edit_first(&mut guard).await
    }

    async fn handle_custom_input(
        &self,
        menu: &mut MenuSession,
        pending: &str,
        text: &str,
        context: CustomInputContext<'_>,
    ) -> Result<()> {
        let parts = pending.split(':').collect::<Vec<_>>();
        if parts.len() != 3 || parts[0] != "custom" {
            return Err(anyhow!("custom login state is invalid"));
        }
        let wizard = self
            .custom_logins
            .get(parts[1])
            .ok_or_else(|| anyhow!("custom login expired; run /login again"))?;
        let mut wizard = wizard.lock().await;
        if !wizard.valid_for(context.user_id, context.scope, &menu.id) {
            return Err(anyhow!(
                "custom login state does not belong to this topic/menu"
            ));
        }
        match parts[2] {
            "endpoint" if wizard.phase == CustomLoginPhase::Endpoint => {
                let endpoint = validate_custom_endpoint(text)?;
                if wizard.endpoint.as_deref() != Some(endpoint.as_str()) {
                    self.clear_custom_wizard_credential(&mut wizard)?;
                }
                wizard.endpoint = Some(endpoint);
                wizard.models.clear();
                wizard.selected_index = None;
                wizard.capability = None;
                wizard.phase = CustomLoginPhase::ApiKey;
                menu.pending_input = Some(format!("custom:{}:api_key", wizard.id));
                menu.current_view = login::api_key_view(&wizard.id);
            }
            "api_key" if wizard.phase == CustomLoginPhase::ApiKey => {
                // Credential messages are an exceptional input class: retire
                // the Telegram copy and scrub Xiao's durable inbox payload as
                // soon as the expected scoped wizard state recognizes it.
                let _ = self
                    .client
                    .delete_message(context.scope.chat_id, context.message_id)
                    .await;
                self.app
                    .storage
                    .scrub_telegram_update_payload(context.update_id)?;
                let key = text.trim();
                if key.is_empty() || key.chars().count() > 16_384 {
                    menu.pending_input = Some(pending.into());
                    return Err(anyhow!("API key is empty or too long"));
                }
                let credential = self.app.auth.create_api_key_credential(
                    "custom",
                    &format!("custom-wizard-{}", wizard.id),
                    key,
                )?;
                if let Err(error) = self
                    .app
                    .storage
                    .set_account_owner(context.principal, &credential.id)
                {
                    let _ = self.app.auth.logout(&credential.id);
                    menu.pending_input = Some(pending.into());
                    return Err(error);
                }
                let replacement = credential.id;
                if let Some(previous) = wizard.credential_ref.replace(replacement.clone()) {
                    if let Err(error) = self.app.auth.logout(&previous) {
                        wizard.credential_ref = Some(previous);
                        let _ = self.app.auth.logout(&replacement);
                        menu.pending_input = Some(pending.into());
                        return Err(error.context("replace transient Custom credential"));
                    }
                }
                self.app.storage.audit(
                    context.principal,
                    "custom_profile_credential_captured",
                    &format!("wizard_id={}", wizard.id),
                )?;
                wizard.phase = CustomLoginPhase::Alias;
                menu.pending_input = Some(format!("custom:{}:alias", wizard.id));
                menu.current_view = login::alias_view(&wizard.id);
            }
            "alias" if wizard.phase == CustomLoginPhase::Alias => {
                let alias = validate_custom_alias(text)?;
                wizard.alias = self.resolve_custom_alias(context.principal, &alias)?;
                menu.current_view = View::info(
                    "CUSTOM LOGIN",
                    "Validating endpoint and discovering models…",
                );
                self.discover_custom_models(&mut wizard).await?;
                menu.pending_input = None;
                menu.current_view = login::model_view(&wizard);
            }
            _ => return Err(anyhow!("custom login input is stale or out of sequence")),
        }
        Ok(())
    }

    fn resolve_custom_alias(&self, principal: &str, candidate: &str) -> Result<String> {
        let raw = candidate.trim();
        let base = if raw.is_empty() {
            "custom"
        } else if let Some((prefix, suffix)) = raw.rsplit_once('_') {
            if !prefix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                prefix
            } else {
                raw
            }
        } else {
            raw
        };

        let store = crate::providers::ProviderProfileStore::new(self.app.storage.clone());
        if store.get_by_alias(principal, base)?.is_none() {
            return Ok(base.to_string());
        }
        let mut suffix = 1usize;
        loop {
            let alias = format!("{base}_{suffix}");
            if store.get_by_alias(principal, &alias)?.is_none() {
                return Ok(alias);
            }
            suffix += 1;
        }
    }

    async fn discover_custom_models(&self, wizard: &mut login::CustomLoginWizard) -> Result<()> {
        let endpoint = wizard
            .endpoint
            .as_deref()
            .ok_or_else(|| anyhow!("custom endpoint is missing"))?;
        // Enter the discovery phase before I/O. A failed request can then be
        // retried or navigated back without confusing it with alias input.
        wizard.phase = CustomLoginPhase::Models;
        wizard.models.clear();
        wizard.selected_index = None;
        wizard.capability = None;
        let headers = std::collections::BTreeMap::new();
        let api_key = match wizard.credential_ref.as_deref() {
            Some(reference) => self
                .app
                .auth
                .credential(reference)?
                .and_then(|credential| credential.api_key),
            None => None,
        };
        let models = crate::ipc::fetch_custom_models(endpoint, &headers, api_key.as_deref())
            .await
            .map_err(|error| anyhow!(classify_custom_error(&error)))?;
        wizard.models = models;
        wizard.page = 1;
        Ok(())
    }

    fn clear_custom_wizard_credential(&self, wizard: &mut login::CustomLoginWizard) -> Result<()> {
        let Some(reference) = wizard.credential_ref.take() else {
            return Ok(());
        };
        if let Err(error) = self.app.auth.logout(&reference) {
            wizard.credential_ref = Some(reference);
            return Err(error.context("clear transient Custom credential"));
        }
        Ok(())
    }

    async fn handle_custom_action(
        &self,
        menu: &mut MenuSession,
        principal: &str,
        command: &str,
    ) -> Result<()> {
        let parts = command
            .trim_start_matches("/_custom:")
            .split(':')
            .collect::<Vec<_>>();
        if parts.len() < 2 {
            return Err(anyhow!("invalid custom login action"));
        }
        let wizard_id = parts[0];
        let wizard = self
            .custom_logins
            .get(wizard_id)
            .ok_or_else(|| anyhow!("custom login expired; run /login again"))?;
        let mut wizard = wizard.lock().await;
        let scope = TelegramScope::new(menu.chat_id, menu.message_thread_id);
        if !wizard.valid_for(menu.owner_user_id, scope, &menu.id) {
            return Err(anyhow!(
                "custom login action has the wrong owner/topic/menu"
            ));
        }
        match parts[1] {
            "skip_key" if wizard.phase == CustomLoginPhase::ApiKey => {
                self.clear_custom_wizard_credential(&mut wizard)?;
                wizard.phase = CustomLoginPhase::Alias;
                menu.pending_input = Some(format!("custom:{wizard_id}:alias"));
                menu.current_view = login::alias_view(wizard_id);
            }
            "default_alias" if wizard.phase == CustomLoginPhase::Alias => {
                wizard.alias = self.resolve_custom_alias(principal, "custom")?;
                self.discover_custom_models(&mut wizard).await?;
                menu.pending_input = None;
                menu.current_view = login::model_view(&wizard);
            }
            "page" if wizard.phase == CustomLoginPhase::Models && parts.len() == 3 => {
                wizard.page = parts[2].parse().unwrap_or(1);
                menu.current_view = login::model_view(&wizard);
            }
            "select" if wizard.phase == CustomLoginPhase::Models && parts.len() == 3 => {
                let index = parts[2]
                    .parse::<usize>()
                    .map_err(|_| anyhow!("invalid model selection"))?;
                if index >= wizard.models.len() {
                    return Err(anyhow!("model selection is out of range"));
                }
                wizard.selected_index = Some(index);
                let model = wizard.models[index].clone();
                let endpoint = wizard
                    .endpoint
                    .as_deref()
                    .ok_or_else(|| anyhow!("custom endpoint is missing"))?;
                let headers = std::collections::BTreeMap::new();
                let api_key = match wizard.credential_ref.as_deref() {
                    Some(reference) => self
                        .app
                        .auth
                        .credential(reference)?
                        .and_then(|credential| credential.api_key),
                    None => None,
                };
                wizard.capability = Some(
                    crate::providers::probe_custom_capabilities(
                        endpoint,
                        &headers,
                        api_key.as_deref(),
                        &wizard.protocol,
                        &model,
                    )
                    .await,
                );
                wizard.phase = CustomLoginPhase::Confirm;
                menu.current_view = login::confirmation_view(&wizard);
            }
            "confirm" if wizard.phase == CustomLoginPhase::Confirm => {
                let mut retries = 0;
                loop {
                    wizard.alias = self.resolve_custom_alias(principal, &wizard.alias)?;
                    match self.commit_custom_login(principal, &wizard).await {
                        Ok(()) => break,
                        Err(error) => {
                            let msg = error.to_string().to_ascii_lowercase();
                            if ((msg.contains("already exists") && msg.contains("alias"))
                                || (msg.contains("unique") && msg.contains("alias")))
                                && retries < 5
                            {
                                retries += 1;
                                continue;
                            }
                            return Err(error);
                        }
                    }
                }
                let model = wizard
                    .selected_index
                    .and_then(|index| wizard.models.get(index))
                    .cloned()
                    .ok_or_else(|| anyhow!("selected model disappeared"))?;
                menu.pending_input = None;
                menu.current_view = View::info(
                    "CUSTOM LOGIN",
                    format!("CONNECTED\n{} · {model}", wizard.alias),
                );
                menu.current_view.actions =
                    vec![vec![Action::command("Model", "/model"), Action::close()]];
                self.custom_logins.remove(wizard_id);
            }
            "retry" => match wizard.phase {
                CustomLoginPhase::Endpoint => {
                    menu.pending_input = Some(format!("custom:{wizard_id}:endpoint"));
                    menu.current_view = login::endpoint_view(wizard_id);
                }
                CustomLoginPhase::ApiKey => {
                    menu.pending_input = Some(format!("custom:{wizard_id}:api_key"));
                    menu.current_view = login::api_key_view(wizard_id);
                }
                CustomLoginPhase::Alias => {
                    menu.pending_input = Some(format!("custom:{wizard_id}:alias"));
                    menu.current_view = login::alias_view(wizard_id);
                }
                CustomLoginPhase::Models => {
                    self.discover_custom_models(&mut wizard).await?;
                    menu.pending_input = None;
                    menu.current_view = login::model_view(&wizard);
                }
                CustomLoginPhase::Confirm => {
                    let mut retries = 0;
                    loop {
                        wizard.alias = self.resolve_custom_alias(principal, &wizard.alias)?;
                        match self.commit_custom_login(principal, &wizard).await {
                            Ok(()) => break,
                            Err(error) => {
                                let msg = error.to_string().to_ascii_lowercase();
                                if ((msg.contains("already exists") && msg.contains("alias"))
                                    || (msg.contains("unique") && msg.contains("alias")))
                                    && retries < 5
                                {
                                    retries += 1;
                                    continue;
                                }
                                return Err(error);
                            }
                        }
                    }
                    let model = wizard
                        .selected_index
                        .and_then(|index| wizard.models.get(index))
                        .cloned()
                        .ok_or_else(|| anyhow!("selected model disappeared"))?;
                    menu.pending_input = None;
                    menu.current_view = View::info(
                        "CUSTOM LOGIN",
                        format!("CONNECTED\n{} · {model}", wizard.alias),
                    );
                    menu.current_view.actions =
                        vec![vec![Action::command("Model", "/model"), Action::close()]];
                    self.custom_logins.remove(wizard_id);
                }
            },
            "edit_endpoint" => {
                // Endpoint edits cross a trust boundary. The next endpoint
                // must explicitly capture or skip its own credential.
                self.clear_custom_wizard_credential(&mut wizard)?;
                wizard.phase = CustomLoginPhase::Endpoint;
                wizard.endpoint = None;
                wizard.models.clear();
                wizard.selected_index = None;
                wizard.capability = None;
                menu.pending_input = Some(format!("custom:{wizard_id}:endpoint"));
                menu.current_view = login::endpoint_view(wizard_id);
            }
            "wizard_back" => match wizard.phase {
                CustomLoginPhase::Endpoint => {
                    menu.pending_input = Some(format!("custom:{wizard_id}:endpoint"));
                    menu.current_view = login::endpoint_view(wizard_id);
                }
                CustomLoginPhase::ApiKey => {
                    wizard.phase = CustomLoginPhase::Endpoint;
                    menu.pending_input = Some(format!("custom:{wizard_id}:endpoint"));
                    menu.current_view = login::endpoint_view(wizard_id);
                }
                CustomLoginPhase::Alias => {
                    wizard.phase = CustomLoginPhase::ApiKey;
                    menu.pending_input = Some(format!("custom:{wizard_id}:api_key"));
                    menu.current_view = login::api_key_view(wizard_id);
                }
                CustomLoginPhase::Models => {
                    wizard.phase = CustomLoginPhase::Alias;
                    menu.pending_input = Some(format!("custom:{wizard_id}:alias"));
                    menu.current_view = login::alias_view(wizard_id);
                }
                CustomLoginPhase::Confirm => {
                    wizard.phase = CustomLoginPhase::Models;
                    menu.pending_input = None;
                    menu.current_view = login::model_view(&wizard);
                }
            },
            _ => return Err(anyhow!("custom login action is stale or out of sequence")),
        }
        Ok(())
    }

    async fn commit_custom_login(
        &self,
        principal: &str,
        wizard: &login::CustomLoginWizard,
    ) -> Result<()> {
        let model = wizard
            .selected_index
            .and_then(|index| wizard.models.get(index))
            .cloned()
            .ok_or_else(|| anyhow!("select a model before confirmation"))?;
        let probe = wizard
            .capability
            .as_ref()
            .ok_or_else(|| anyhow!("selected model capability was not probed"))?;
        let probed_at = chrono::Utc::now().to_rfc3339();
        let context = self
            .app
            .sessions
            .context_for_telegram(principal, wizard.scope)?;
        let profile_service = crate::providers::CustomProfileService::new(
            self.app.storage.clone(),
            self.app.auth.secrets().clone(),
        );
        let profile_models = wizard
            .models
            .iter()
            .map(|candidate| {
                if candidate == &model {
                    crate::providers::profile_model_from_probe(
                        "pending-custom-profile",
                        candidate,
                        probe,
                        &probed_at,
                    )
                } else {
                    crate::storage::ProviderProfileModelRecord {
                        profile_id: "pending-custom-profile".into(),
                        model_id: candidate.clone(),
                        text_capable: true,
                        vision_capable: false,
                        file_input_capable: false,
                        native_tools: false,
                        structured_output: false,
                        continuation: false,
                        native_tools_state: "unknown".into(),
                        structured_output_state: "unknown".into(),
                        continuation_state: "unknown".into(),
                        vision_state: "unknown".into(),
                        file_input_state: "unknown".into(),
                        model_discovery: true,
                        tool_protocol: "chat_only".into(),
                        evidence: "discovered but not capability-probed".into(),
                        probe_status: "unprobed".into(),
                        probe_version: 1,
                        probed_at: probed_at.clone(),
                    }
                }
            })
            .collect::<Vec<_>>();
        profile_service.create_profile_with_models_and_activate_session_with_credential_ref(
            principal,
            &wizard.alias,
            wizard
                .endpoint
                .as_deref()
                .ok_or_else(|| anyhow!("custom endpoint is missing"))?,
            &wizard.protocol,
            std::collections::BTreeMap::new(),
            wizard.credential_ref.as_deref(),
            &profile_models,
            &context.active.id,
            &model,
        )?;
        Ok(())
    }

    fn safe_artifact(&self, path: &std::path::Path) -> bool {
        let Ok(path) = path.canonicalize() else {
            return false;
        };
        if !path.is_file() {
            return false;
        }
        let environment = self.app.runtime.environment();
        path.starts_with(&environment.data_root)
            || environment
                .termux
                .as_ref()
                .is_some_and(|termux| path.starts_with(&termux.home))
    }

    async fn send_menu(
        &self,
        scope: TelegramScope,
        user_id: i64,
        view: View,
    ) -> Result<Arc<tokio::sync::Mutex<MenuSession>>> {
        let menu = self.menus.prepare_scoped(scope, user_id, view);
        let (id, rendered, markup) = {
            let guard = menu.lock().await;
            (
                guard.id.clone(),
                rich::render(&guard.current_view, false),
                keyboard(&guard.current_view, &guard.id, guard.revision),
            )
        };
        let sent = match self
            .client
            .send_rich_scoped(scope, rendered, Some(markup.clone()))
            .await
        {
            Ok(message) => message.message_id,
            Err(_) => {
                self.client
                    .send_plain_scoped(
                        scope,
                        &rich::plain(&menu.lock().await.current_view),
                        Some(markup),
                    )
                    .await?
                    .message_id
            }
        };
        menu.lock().await.message_id = sent;
        self.menus.insert(menu.clone(), id);
        Ok(menu)
    }

    async fn handle_callback(&self, callback: CallbackQuery) -> Result<()> {
        let Some(message) = callback.message.as_ref() else {
            return Ok(());
        };
        let callback_scope = message.scope();
        if !self
            .allowed(message.chat.id, Some(callback.from.id), &message.chat.kind)
            .await
        {
            let _ = self
                .client
                .answer_callback(&callback.id, Some("Not authorized."), true)
                .await;
            return Ok(());
        }
        let Some(data) = callback.data.as_deref() else {
            let _ = self.client.answer_callback(&callback.id, None, false).await;
            return Ok(());
        };
        let Ok((menu_id, expected_revision, index)) = parse_callback(data) else {
            let _ = self.client.answer_callback(&callback.id, None, false).await;
            return Ok(());
        };
        let Some(menu) = self.menus.get(&menu_id) else {
            let _ = self
                .client
                .answer_callback(
                    &callback.id,
                    Some("Menu expired. Run the command again."),
                    false,
                )
                .await;
            return Ok(());
        };
        // Stop Telegram's callback spinner before waiting on the per-menu serialization lock.
        // Stale/ownership checks below still prevent mutation even though the UX ACK is immediate.
        let _ = self.client.answer_callback(&callback.id, None, false).await;
        let mut guard = menu.lock().await;
        if guard.owner_user_id != callback.from.id
            || guard.chat_id != callback_scope.chat_id
            || guard.message_thread_id != callback_scope.message_thread_id
            || guard.expires_at <= std::time::Instant::now()
        {
            return Ok(());
        }
        if guard.revision != expected_revision {
            return Ok(());
        }
        let Some(action) = action_at(&guard.current_view, index) else {
            return Ok(());
        };
        match action.target {
            ActionTarget::Noop => return Ok(()),
            ActionTarget::Close => {
                let principal = self
                    .app
                    .resolve_telegram_owner(guard.owner_user_id)?
                    .owner_id;
                let configured = self
                    .app
                    .config
                    .read()
                    .await
                    .telegram
                    .ui
                    .menu_close_behavior
                    .clone();
                let behavior = self
                    .app
                    .storage
                    .setting(&format!("telegram.menu_close_behavior:{principal}"))?
                    .unwrap_or(configured);
                if behavior == "delete_message" {
                    let _ = self
                        .client
                        .delete_message(guard.chat_id, guard.message_id)
                        .await;
                } else {
                    let _ = self
                        .client
                        .edit_markup(
                            guard.chat_id,
                            guard.message_id,
                            Some(json!({"inline_keyboard":[]})),
                        )
                        .await;
                }
                for reference in self.custom_logins.remove_uncommitted_by_menu(&menu_id) {
                    let _ = self.app.auth.logout(&reference);
                }
                self.menus.remove(&menu_id);
                return Ok(());
            }
            ActionTarget::Back => {
                guard.pending_input = None;
                if let Some(previous) = guard.history.pop() {
                    guard.current_view = previous;
                    guard.revision += 1;
                    self.edit_first(&mut guard).await?;
                }
                return Ok(());
            }
            ActionTarget::Url(_) => return Ok(()),
            ActionTarget::Command(command) => {
                let principal = self
                    .app
                    .resolve_telegram_owner(guard.owner_user_id)?
                    .owner_id;
                if let Some((approve, approval_id)) = parse_internal_approval_command(&command) {
                    let decided =
                        self.app
                            .storage
                            .decide_approval(&principal, approval_id, approve)?;
                    if decided {
                        self.app.storage.audit(
                            &principal,
                            "telegram_contextual_approval_decided",
                            &format!(
                                "approval_id={approval_id};decision={}",
                                if approve { "approved" } else { "denied" }
                            ),
                        )?;
                    }
                    guard.current_view = View::info(
                        "APPROVAL",
                        if decided {
                            if approve {
                                "Approved once. The active run may now continue."
                            } else {
                                "Denied. The active run will receive the denial."
                            }
                        } else {
                            "This approval is no longer pending or has expired."
                        },
                    );
                    guard.pending_input = None;
                    guard.revision += 1;
                    self.edit_first(&mut guard).await?;
                    return Ok(());
                }
                if command.starts_with("/_custom:") {
                    if let Err(error) = self
                        .handle_custom_action(&mut guard, &principal, &command)
                        .await
                    {
                        let wizard_id = command
                            .trim_start_matches("/_custom:")
                            .split(':')
                            .next()
                            .unwrap_or_default();
                        let msg = error.to_string().to_ascii_lowercase();
                        let is_alias_collision = (msg.contains("already exists")
                            && msg.contains("alias"))
                            || (msg.contains("unique") && msg.contains("alias"));
                        if is_alias_collision {
                            if let Some(wizard) = self.custom_logins.get(wizard_id) {
                                if let Ok(mut state) = wizard.try_lock() {
                                    state.phase = login::CustomLoginPhase::Alias;
                                    let alias = state.alias.clone();
                                    guard.pending_input = Some(format!("custom:{wizard_id}:alias"));
                                    guard.current_view =
                                        login::alias_collision_view(wizard_id, &alias);
                                } else {
                                    let endpoint = self
                                        .custom_logins
                                        .get(wizard_id)
                                        .and_then(|w| w.try_lock().ok()?.endpoint.clone());
                                    guard.current_view = login::failure_view(
                                        wizard_id,
                                        endpoint.as_deref(),
                                        &classify_custom_error(&error),
                                    );
                                }
                            } else {
                                let endpoint = self
                                    .custom_logins
                                    .get(wizard_id)
                                    .and_then(|wizard| wizard.try_lock().ok()?.endpoint.clone());
                                guard.current_view = login::failure_view(
                                    wizard_id,
                                    endpoint.as_deref(),
                                    &classify_custom_error(&error),
                                );
                            }
                        } else {
                            let endpoint = self
                                .custom_logins
                                .get(wizard_id)
                                .and_then(|wizard| wizard.try_lock().ok()?.endpoint.clone());
                            guard.current_view = login::failure_view(
                                wizard_id,
                                endpoint.as_deref(),
                                &classify_custom_error(&error),
                            );
                        }
                    }
                    guard.revision += 1;
                    self.advance_menu_prompt(&mut guard).await?;
                    return Ok(());
                }
                let fast = command
                    .split_whitespace()
                    .next()
                    .is_some_and(|x| matches!(x.split('@').next(), Some("/stop") | Some("/retry")));
                let result = if fast {
                    self.app
                        .commands
                        .execute_callback_in_telegram_scope(
                            &principal,
                            callback_scope,
                            &command,
                            None,
                        )
                        .await?
                } else {
                    let lane = self.principal_lock(&principal);
                    let _guard = lane.lock().await;
                    self.app
                        .commands
                        .execute_callback_in_telegram_scope(
                            &principal,
                            callback_scope,
                            &command,
                            None,
                        )
                        .await?
                };
                let (next, pending) = match result {
                    CommandResult::InputRequest {
                        view,
                        command_prefix,
                    } => (view, Some(command_prefix)),
                    CommandResult::StartCustomLogin => {
                        let wizard = self.custom_logins.begin(
                            callback_scope,
                            callback.from.id,
                            guard.id.clone(),
                        );
                        let id = wizard.lock().await.id.clone();
                        (
                            login::endpoint_view(&id),
                            Some(format!("custom:{id}:endpoint")),
                        )
                    }
                    CommandResult::Agent(answer) => (agent_final_view(answer), None),
                    other => (result_view(other)?, None),
                };
                let previous = std::mem::replace(&mut guard.current_view, next);
                guard.history.push(previous);
                guard.pending_input = pending;
                guard.revision += 1;
                self.edit_first(&mut guard).await?;
            }
        }
        Ok(())
    }

    async fn edit_first(&self, guard: &mut MenuSession) -> Result<()> {
        let markup = keyboard(&guard.current_view, &guard.id, guard.revision);
        let rendered = rich::render(&guard.current_view, false);
        let plain = rich::plain(&guard.current_view);
        menu::edit_first(&self.client, guard, rendered, plain, markup).await
    }

    /// Wizard state transitions are chronological: retire the previous
    /// keyboard, then send a distinct prompt message for the new state.
    async fn advance_menu_prompt(&self, guard: &mut MenuSession) -> Result<()> {
        if guard.message_id != 0 {
            let _ = self
                .client
                .edit_markup(
                    guard.chat_id,
                    guard.message_id,
                    Some(json!({"inline_keyboard":[]})),
                )
                .await;
        }
        let scope = TelegramScope::new(guard.chat_id, guard.message_thread_id);
        let markup = keyboard(&guard.current_view, &guard.id, guard.revision);
        guard.message_id = self
            .send_view(scope, &guard.current_view, Some(markup))
            .await?;
        Ok(())
    }
}

fn validate_custom_endpoint(value: &str) -> Result<String> {
    let value = value.trim();
    if value.chars().count() > 2_048 {
        return Err(anyhow!("custom endpoint is too long"));
    }
    let parsed = url::Url::parse(value).map_err(|_| anyhow!("invalid custom endpoint URL"))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(anyhow!(
            "custom endpoint must be an HTTP(S) URL with a host and no embedded credentials"
        ));
    }
    if parsed.scheme() == "http" && !is_private_http_host(parsed.host_str().unwrap_or_default()) {
        return Err(anyhow!(
            "plain HTTP custom endpoints are allowed only on localhost or private/link-local IP addresses"
        ));
    }
    Ok(value.trim_end_matches('/').to_owned())
}

fn is_private_http_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|address| match address {
            std::net::IpAddr::V4(address) => {
                address.is_private() || address.is_loopback() || address.is_link_local()
            }
            std::net::IpAddr::V6(address) => {
                address.is_loopback()
                    || address.is_unique_local()
                    || address.is_unicast_link_local()
            }
        })
}

fn validate_custom_alias(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > 48
        || !value
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '-' | '_' | ' '))
    {
        return Err(anyhow!(
            "custom alias must be 1–48 letters, numbers, spaces, hyphens, or underscores"
        ));
    }
    Ok(value.to_ascii_lowercase().replace(' ', "-"))
}

fn classify_custom_error(error: &anyhow::Error) -> String {
    let raw = crate::security::redact::redact_text(&error.to_string());
    let lower = raw.to_ascii_lowercase();
    let category = if lower.contains("connection refused") {
        "Connection refused"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "Connection timed out"
    } else if lower.contains("tls") || lower.contains("certificate") {
        "TLS validation failed"
    } else if lower.contains("401") {
        "HTTP 401: authentication rejected"
    } else if lower.contains("403") {
        "HTTP 403: access forbidden"
    } else if lower.contains("404") {
        "HTTP 404: models endpoint not found"
    } else if lower.contains("json") || lower.contains("decode") || lower.contains("parse") {
        "Endpoint returned invalid JSON"
    } else if lower.contains("no model") || lower.contains("model id") {
        "Endpoint returned an empty model list"
    } else if lower.contains("endpoint") || lower.contains("url") {
        "Endpoint is invalid or unsupported"
    } else {
        "Provider could not be reached or validated"
    };
    format!("{category}. {}", raw.chars().take(240).collect::<String>())
}

fn paginate_final_view(view: &View, max_chars: usize) -> Vec<View> {
    let max_chars = max_chars.max(256);
    let mut pages = Vec::new();
    let mut blocks = Vec::new();
    let mut used = 0usize;
    for block in &view.blocks {
        let single = View {
            title: None,
            blocks: vec![block.clone()],
            actions: vec![],
            side_mode: false,
        };
        let plain = rich::plain(&single);
        let size = plain.chars().count();
        if size > max_chars {
            if !blocks.is_empty() {
                pages.push(View {
                    title: None,
                    blocks: std::mem::take(&mut blocks),
                    actions: vec![],
                    side_mode: view.side_mode,
                });
                used = 0;
            }
            let chars = plain.chars().collect::<Vec<_>>();
            for chunk in chars.chunks(max_chars) {
                pages.push(View {
                    title: None,
                    blocks: vec![Block::Paragraph {
                        text: chunk.iter().collect(),
                    }],
                    actions: vec![],
                    side_mode: view.side_mode,
                });
            }
            continue;
        }
        if !blocks.is_empty() && used + size > max_chars {
            pages.push(View {
                title: None,
                blocks: std::mem::take(&mut blocks),
                actions: vec![],
                side_mode: view.side_mode,
            });
            used = 0;
        }
        blocks.push(block.clone());
        used += size;
    }
    if !blocks.is_empty() || pages.is_empty() {
        pages.push(View {
            title: None,
            blocks,
            actions: vec![],
            side_mode: view.side_mode,
        });
    }
    if let Some(first) = pages.first_mut() {
        first.title = view.title.clone();
    }
    pages
}

struct ProgressAggregator {
    items: Vec<ProgressItem>,
    next_id: u64,
    detail: String,
    stream_chunks: usize,
    visible_text: String,
}

struct ToolProgress {
    activity: ProgressActivity,
    icon: ProgressIcon,
    active: String,
    completed: String,
    failed: String,
}

const PROGRESS_CHAR_BUDGET: usize = 3_500;

impl ProgressAggregator {
    fn new(detail: String) -> Self {
        Self {
            items: vec![],
            next_id: 1,
            detail,
            stream_chunks: 0,
            visible_text: String::new(),
        }
    }

    fn push(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::GenerationStarted => {
                self.set_active("Thinking".into(), ProgressActivity::Thinking)
            }
            AgentEvent::Status(text) => {
                let (label, activity) = status_progress(&text);
                self.set_active(label, activity);
            }
            AgentEvent::ToolStarted(tool) => {
                let progress = tool_progress(&tool);
                self.set_active_for_tool(progress.active, progress.activity, tool, None);
            }
            AgentEvent::ToolStartedWithId { tool, call_id } => {
                let progress = tool_progress(&tool);
                self.set_active_for_tool(progress.active, progress.activity, tool, Some(call_id));
            }
            AgentEvent::ToolCompleted { tool, summary } => {
                self.complete_tool(&tool, None, &summary);
            }
            AgentEvent::ToolCompletedWithId {
                tool,
                call_id,
                summary,
            } => {
                self.complete_tool(&tool, Some(call_id), &summary);
            }
            AgentEvent::ApprovalRequested {
                tool,
                call_id,
                summary,
                ..
            } => self.await_approval(&tool, &call_id, &summary),
            AgentEvent::StreamChunk { .. } => self.stream_chunk(),
            AgentEvent::TextDelta(delta) => {
                self.visible_text.push_str(&delta);
                if self.visible_text.chars().count() > 3_000 {
                    self.visible_text = self
                        .visible_text
                        .chars()
                        .rev()
                        .take(3_000)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                }
                self.stream_chunk();
            }
            AgentEvent::GenerationCompleted => {
                self.set_active("Finishing response".into(), ProgressActivity::Writing)
            }
            AgentEvent::GenerationFailed(error) => self.fail(&error),
        }
    }

    fn set_active(&mut self, label: String, activity: ProgressActivity) {
        self.set_active_entry(
            label,
            activity,
            ProgressIcon::from_activity(activity),
            None,
            None,
        );
    }

    fn set_active_for_tool(
        &mut self,
        label: String,
        activity: ProgressActivity,
        tool: String,
        correlation: Option<String>,
    ) {
        let progress = tool_progress(&tool);
        self.set_active_entry(
            label,
            activity,
            progress.icon,
            Some(normalize_tool_name(&tool)),
            correlation,
        );
    }

    fn set_active_entry(
        &mut self,
        label: String,
        activity: ProgressActivity,
        icon: ProgressIcon,
        tool: Option<String>,
        correlation: Option<String>,
    ) {
        let same_action = self.items.iter().rposition(|item| {
            item.state == ProgressState::Active
                && match (
                    item.action_key.as_deref(),
                    tool.as_deref(),
                    item.correlation_id.as_deref(),
                    correlation.as_deref(),
                ) {
                    (Some(current), Some(next), Some(current_id), Some(next_id)) => {
                        current == next && current_id == next_id
                    }
                    (Some(current), Some(next), None, None) => current == next,
                    (None, None, None, None) => item.activity == activity,
                    _ => false,
                }
        });
        if let Some(index) = same_action {
            let item = &mut self.items[index];
            item.label = label;
            item.activity = activity;
            item.icon = icon;
            item.action_key = tool;
            item.correlation_id = correlation;
            return;
        }

        // A timeline has one current active row. Starting a genuinely new
        // action closes the previous row in place; it never removes history.
        if let Some(index) = self
            .items
            .iter()
            .rposition(|item| item.state == ProgressState::Active)
        {
            let previous_tool = self.items[index].action_key.clone();
            let item = &mut self.items[index];
            item.state = ProgressState::Done;
            if let Some(previous_tool) = previous_tool {
                item.label = tool_progress(&previous_tool).completed;
            }
        }
        self.items.push(ProgressItem {
            id: self.next_id,
            state: ProgressState::Active,
            activity,
            icon,
            action_key: tool,
            correlation_id: correlation,
            summary: None,
            label,
        });
        self.next_id = self.next_id.saturating_add(1);
        self.trim();
    }

    fn complete_tool(&mut self, tool: &str, correlation: Option<String>, summary: &str) {
        let progress = tool_progress(tool);
        let safe_summary = safe_progress(summary);
        let lower = safe_summary.to_ascii_lowercase();
        let failed = lower.starts_with("failed")
            || lower.starts_with("error")
            || lower.starts_with("cancelled");
        let mut label = if failed {
            progress.failed
        } else {
            progress.completed
        };
        if failed {
            let detail = safe_summary
                .strip_prefix("failed: ")
                .or_else(|| safe_summary.strip_prefix("error: "))
                .or_else(|| safe_summary.strip_prefix("cancelled: "))
                .unwrap_or(&safe_summary);
            if !detail.is_empty() && detail != "failed" {
                label.push_str(" · ");
                label.push_str(detail);
            }
        }
        if self.detail == "detailed" && !matches!(safe_summary.as_str(), "completed" | "failed") {
            let detail = safe_summary
                .strip_prefix("failed: ")
                .unwrap_or(&safe_summary);
            label.push_str(" · ");
            label.push_str(detail);
        }
        let state = if failed {
            ProgressState::Failed
        } else {
            ProgressState::Done
        };
        let tool_key = normalize_tool_name(tool);
        let index = if let Some(correlation) = correlation.as_deref() {
            // Correlation IDs are the only safe way to complete an older row
            // after another action has started. The tool name is checked too
            // so a malformed provider event cannot cross tool boundaries.
            self.items.iter().position(|item| {
                item.action_key.as_deref() == Some(tool_key.as_str())
                    && item.correlation_id.as_deref() == Some(correlation)
            })
        } else {
            // Legacy events without a call ID are intentionally limited to
            // the current active row. They cannot complete a newer/different
            // action or guess at a historical row.
            self.items.iter().rposition(|item| {
                item.state == ProgressState::Active
                    && item.action_key.as_deref() == Some(tool_key.as_str())
                    && item.correlation_id.is_none()
            })
        };
        let Some(index) = index else {
            return;
        };
        if let Some(item) = self.items.get_mut(index) {
            item.state = state;
            item.activity = progress.activity;
            item.icon = progress.icon;
            item.label = label;
            item.summary = Some(safe_summary.clone());
        }
        self.trim();
    }

    fn await_approval(&mut self, tool: &str, call_id: &str, summary: &str) {
        let progress = tool_progress(tool);
        let detail = safe_progress(summary);
        let label = if detail.is_empty() {
            format!("Awaiting approval · {}", progress.active)
        } else {
            safe_progress(&format!(
                "Awaiting approval · {} · {detail}",
                progress.active
            ))
        };
        self.set_active_for_tool(
            label,
            progress.activity,
            tool.to_owned(),
            Some(call_id.to_owned()),
        );
    }

    fn stream_chunk(&mut self) {
        self.stream_chunks += 1;
        if self.items.last().is_some_and(|item| {
            item.state == ProgressState::Active
                && !matches!(
                    item.activity,
                    ProgressActivity::Thinking | ProgressActivity::Writing
                )
        }) {
            return;
        }
        if let Some(active) = self.items.last_mut().filter(|item| {
            item.state == ProgressState::Active && item.activity == ProgressActivity::Thinking
        }) {
            // The first provider chunk is the same generation action changing
            // phase, not a new row. This keeps streaming as one Writing item.
            active.activity = ProgressActivity::Writing;
            active.icon = ProgressIcon::Writing;
            active.label = if self.detail == "detailed" {
                format!("Writing response · {} chunks", self.stream_chunks)
            } else {
                "Writing response".into()
            };
            return;
        }
        if let Some(active) = self.items.last_mut().filter(|item| {
            item.state == ProgressState::Active && item.activity == ProgressActivity::Writing
        }) {
            if self.detail == "detailed" && self.stream_chunks.is_multiple_of(8) {
                active.label = format!("Writing response · {} chunks", self.stream_chunks);
            }
            return;
        }
        self.set_active("Writing response".into(), ProgressActivity::Writing);
    }

    fn fail(&mut self, error: &str) {
        let label = if self.detail == "detailed" {
            format!("Request failed · {}", safe_progress(error))
        } else {
            "Request failed".into()
        };
        if let Some(active) = self
            .items
            .iter_mut()
            .rev()
            .find(|item| item.state == ProgressState::Active)
        {
            active.state = ProgressState::Failed;
            active.label = label;
            active.summary = Some(safe_progress(error));
        } else {
            self.items.push(ProgressItem {
                id: self.next_id,
                state: ProgressState::Failed,
                activity: ProgressActivity::Thinking,
                icon: ProgressIcon::Thinking,
                action_key: None,
                correlation_id: None,
                summary: Some(safe_progress(error)),
                label,
            });
            self.next_id = self.next_id.saturating_add(1);
        }
        self.trim();
    }

    fn trim(&mut self) {
        let max = match self.detail.as_str() {
            "minimal" => 1,
            "detailed" => 30,
            _ => 24,
        };
        if self.items.len() > max {
            let remove = self.items.len() - max;
            self.items.drain(..remove);
        }
    }

    fn view(&self) -> View {
        let mut items = self.items.clone();
        if items.is_empty() {
            items.push(ProgressItem {
                id: self.next_id,
                state: ProgressState::Active,
                activity: ProgressActivity::Thinking,
                icon: ProgressIcon::Thinking,
                action_key: Some("generation".into()),
                correlation_id: None,
                summary: None,
                label: "Thinking".into(),
            });
        }
        // Keep the current active row and the newest history while satisfying
        // the Telegram draft budget. Labels are already bounded, so this is
        // only reached for an unusually long timeline.
        while items.len() > 1 && progress_text_length(&items) > PROGRESS_CHAR_BUDGET {
            let active_index = items
                .iter()
                .rposition(|item| item.state == ProgressState::Active)
                .unwrap_or(items.len().saturating_sub(1));
            if active_index == 0 {
                break;
            }
            items.remove(0);
        }
        if progress_text_length(&items) > PROGRESS_CHAR_BUDGET {
            let index = items
                .iter()
                .rposition(|item| item.state == ProgressState::Active)
                .unwrap_or_else(|| items.len().saturating_sub(1));
            if let Some(active) = items.get_mut(index) {
                active.label = safe_progress(&active.label);
            }
        }
        let mut blocks = vec![Block::Progress { items }];
        if !self.visible_text.is_empty() {
            blocks.push(Block::Paragraph {
                text: self.visible_text.clone(),
            });
        }
        View {
            title: None,
            blocks,
            actions: vec![],
            side_mode: false,
        }
    }
}

fn progress_text_length(items: &[ProgressItem]) -> usize {
    items
        .iter()
        .map(|item| item.label.chars().count() + 4)
        .sum::<usize>()
        .saturating_add(items.len().saturating_sub(1))
}

fn status_progress(value: &str) -> (String, ProgressActivity) {
    let safe = safe_progress(value);
    let normalized = safe.to_ascii_lowercase();
    if normalized.contains("refresh") {
        ("Refreshing session".into(), ProgressActivity::Analyzing)
    } else if normalized.contains("web") && normalized.contains("search") {
        ("Searching the web".into(), ProgressActivity::Searching)
    } else if normalized.contains("fetch") || normalized.contains("extract") {
        ("Fetching a page".into(), ProgressActivity::Fetching)
    } else if normalized.contains("image")
        || normalized.contains("video")
        || normalized.contains("audio")
    {
        ("Processing media".into(), ProgressActivity::Media)
    } else if normalized.contains("tool") {
        ("Preparing a tool".into(), ProgressActivity::Tool)
    } else if normalized.contains("final") || normalized.contains("completed") {
        ("Finishing response".into(), ProgressActivity::Writing)
    } else if normalized.contains("generating")
        || normalized.contains("preparing")
        || normalized.contains("sending request")
    {
        ("Thinking".into(), ProgressActivity::Thinking)
    } else {
        (safe, ProgressActivity::Analyzing)
    }
}

fn tool_progress(tool: &str) -> ToolProgress {
    let normalized = normalize_tool_name(tool);
    let web = normalized.contains("web")
        || normalized.contains("browser")
        || normalized.contains("http")
        || normalized.contains("url");
    if normalized.contains("search") {
        let scope = if web { "the web" } else { "files" };
        ToolProgress {
            activity: ProgressActivity::Searching,
            icon: if web {
                ProgressIcon::WebSearch
            } else {
                ProgressIcon::FileSearch
            },
            active: format!("Searching {scope}"),
            completed: format!("Searched {scope}"),
            failed: if web {
                "Web search failed".into()
            } else {
                "File search failed".into()
            },
        }
    } else if normalized.contains("fetch")
        || normalized.contains("extract")
        || normalized.contains("download")
        || normalized.contains("open_url")
    {
        ToolProgress {
            activity: ProgressActivity::Fetching,
            icon: if normalized.contains("read") || normalized.contains("document") {
                ProgressIcon::DocumentRead
            } else {
                ProgressIcon::Fetching
            },
            active: "Fetching a page".into(),
            completed: "Fetched the page".into(),
            failed: "Page fetch failed".into(),
        }
    } else if normalized.contains("image")
        || normalized.contains("video")
        || normalized.contains("audio")
        || normalized.contains("media")
    {
        ToolProgress {
            activity: ProgressActivity::Media,
            icon: if normalized.contains("audio") {
                ProgressIcon::Audio
            } else if normalized.contains("video") {
                ProgressIcon::Video
            } else {
                ProgressIcon::ImageInspect
            },
            active: "Processing media".into(),
            completed: "Processed media".into(),
            failed: "Media processing failed".into(),
        }
    } else if normalized.contains("code")
        || normalized.contains("shell")
        || normalized.contains("exec")
        || normalized.contains("terminal")
        || normalized.contains("command")
        || normalized.contains("patch")
        || normalized.contains("edit")
        || normalized.contains("write")
    {
        ToolProgress {
            activity: ProgressActivity::Coding,
            icon: if normalized.contains("edit") || normalized.contains("patch") {
                ProgressIcon::Editing
            } else if normalized.contains("shell")
                || normalized.contains("exec")
                || normalized.contains("terminal")
                || normalized.contains("command")
            {
                ProgressIcon::Terminal
            } else {
                ProgressIcon::Coding
            },
            active: "Working with code".into(),
            completed: "Finished code work".into(),
            failed: "Code work failed".into(),
        }
    } else if normalized.contains("context")
        || normalized.contains("memory")
        || normalized.contains("read")
        || normalized.contains("inspect")
        || normalized.contains("analy")
    {
        ToolProgress {
            activity: ProgressActivity::Analyzing,
            icon: if normalized.contains("read") || normalized.contains("inspect") {
                ProgressIcon::DocumentRead
            } else {
                ProgressIcon::Analyzing
            },
            active: "Checking conversation context".into(),
            completed: "Checked conversation context".into(),
            failed: "Context check failed".into(),
        }
    } else {
        let name = humanize_tool_name(tool);
        ToolProgress {
            activity: ProgressActivity::Tool,
            icon: if normalized.contains("install") {
                ProgressIcon::Installing
            } else if normalized.contains("test") {
                ProgressIcon::Testing
            } else {
                ProgressIcon::Tool
            },
            active: format!("Running {name}"),
            completed: format!("Ran {name}"),
            failed: format!("{name} failed"),
        }
    }
}

fn humanize_tool_name(tool: &str) -> String {
    let name = safe_progress(tool)
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        "tool".into()
    } else {
        name
    }
}

fn normalize_tool_name(tool: &str) -> String {
    tool.trim().to_ascii_lowercase().replace('-', "_")
}

fn parse_internal_approval_command(command: &str) -> Option<(bool, &str)> {
    let parts = command.trim().split(':').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != "/_approval" {
        return None;
    }
    let approve = match parts[1] {
        "approve" => true,
        "deny" => false,
        _ => return None,
    };
    let id = parts[2];
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    valid.then_some((approve, id))
}

fn is_stop_command(text: &str) -> bool {
    text.split_whitespace()
        .next()
        .is_some_and(|command| matches!(command.split('@').next(), Some("/stop")))
}

fn safe_progress(value: &str) -> String {
    let mut s = crate::security::redact::redact_text(value).replace('\n', " ");
    if s.chars().count() > 140 {
        s = s.chars().take(137).collect::<String>() + "…";
    }
    s
}

fn agent_final_view(answer: AgentAnswer) -> View {
    View::from_markdown(&answer.final_answer, answer.side_mode)
}

fn result_view(result: CommandResult) -> Result<View> {
    match result {
        CommandResult::InfoView(v)
        | CommandResult::ManagerView(v)
        | CommandResult::Confirmation(v) => Ok(v),
        CommandResult::InputRequest { view, .. } => Ok(view),
        CommandResult::NoContent => Ok(View::default()),
        _ => Err(anyhow!("result requires specialized renderer")),
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn setmodel_callback_persists_once_and_does_not_wait_for_slow_probe() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::AppConfig::default();
        cfg.storage.database = temp.path().join("xiao.db");
        cfg.paths.data_dir = temp.path().join("data");
        cfg.paths.logs_dir = temp.path().join("logs");
        cfg.paths.secrets_dir = temp.path().join("secrets");
        let app = crate::app::AppState::build(cfg).await.unwrap();

        let owner = "owner1";
        let scope = TelegramScope::new(100, None);
        let session = app.sessions.ensure_telegram_session(owner, scope).unwrap();

        let profiles = crate::providers::ProviderProfileStore::new(app.storage.clone());
        let test_model_rec = crate::storage::ProviderProfileModelRecord {
            profile_id: "test".into(),
            model_id: "test_model".into(),
            text_capable: true,
            vision_capable: false,
            file_input_capable: false,
            native_tools: false,
            structured_output: false,
            continuation: false,
            native_tools_state: "unknown".into(),
            structured_output_state: "unknown".into(),
            continuation_state: "unknown".into(),
            vision_state: "unknown".into(),
            file_input_state: "unknown".into(),
            model_discovery: false,
            tool_protocol: "chat_only".into(),
            evidence: "test".into(),
            probe_status: "unprobed".into(),
            probe_version: 1,
            probed_at: chrono::Utc::now().to_rfc3339(),
        };
        let mut slow_model_rec = test_model_rec.clone();
        slow_model_rec.model_id = "slow_model".into();

        profiles
            .create_with_models_and_activate_session(
                crate::storage::ProviderProfileInput {
                    profile_id: None,
                    owner_id: owner.into(),
                    alias: "custom".into(),
                    endpoint: "https://test".into(),
                    protocol: "openai_chat_completions".into(),
                    credential_ref: None,
                    api_key_ref: None,
                    safe_headers_json: "{}".into(),
                    secret_headers_ref: None,
                },
                &[test_model_rec, slow_model_rec],
                &session.id,
                "test_model",
            )
            .unwrap();

        // Emulate SetModel callback
        let start = std::time::Instant::now();
        let cmd = crate::command::Command::SetModel {
            model: "slow_model".into(),
        };
        let _ = app
            .commands
            .execute_in_scope(owner, Some(scope), cmd)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        // Ensure it doesn't wait (should be very fast, well under 500ms)
        assert!(
            elapsed.as_millis() < 500,
            "SetModel should not wait for slow probe"
        );

        // Ensure the model was persisted
        let active = app
            .sessions
            .context_for_telegram(owner, scope)
            .unwrap()
            .active;
        assert_eq!(active.model, "slow_model");

        // Duplicate call is idempotent
        let cmd_dup = crate::command::Command::SetModel {
            model: "slow_model".into(),
        };
        let _ = app
            .commands
            .execute_in_scope(owner, Some(scope), cmd_dup)
            .await
            .unwrap();
        let active_dup = app
            .sessions
            .context_for_telegram(owner, scope)
            .unwrap()
            .active;
        assert_eq!(active_dup.model, "slow_model");
    }

    #[tokio::test]
    async fn custom_alias_resolution_fills_gaps_and_respects_owner_scope() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::AppConfig::default();
        cfg.storage.database = temp.path().join("xiao.db");
        cfg.paths.data_dir = temp.path().join("data");
        cfg.paths.logs_dir = temp.path().join("logs");
        cfg.paths.secrets_dir = temp.path().join("secrets");
        let app = crate::app::AppState::build(cfg).await.unwrap();
        let custom_logins = Arc::new(CustomLoginStore::new(std::time::Duration::from_secs(60)));
        let tg = TelegramAdapter {
            app: app.clone(),
            client: TelegramClient::with_base("test-token".into(), "http://127.0.0.1:9".into())
                .unwrap(),
            menus: Arc::new(MenuStore::new(Duration::from_secs(60))),
            custom_logins,
            principal_locks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            active_work: Arc::new(std::sync::Mutex::new(HashMap::new())),
        };

        let owner1 = "owner1";
        let owner2 = "owner2";
        let session1 = app.sessions.ensure_default_session(owner1).unwrap();

        let p1 = tg.resolve_custom_alias(owner1, "custom").unwrap();
        assert_eq!(p1, "custom");

        let model_rec = crate::storage::ProviderProfileModelRecord {
            profile_id: "test".into(),
            model_id: "m".into(),
            text_capable: true,
            vision_capable: false,
            file_input_capable: false,
            native_tools: false,
            structured_output: false,
            continuation: false,
            native_tools_state: "unknown".into(),
            structured_output_state: "unknown".into(),
            continuation_state: "unknown".into(),
            vision_state: "unknown".into(),
            file_input_state: "unknown".into(),
            model_discovery: false,
            tool_protocol: "chat_only".into(),
            evidence: "test".into(),
            probe_status: "unprobed".into(),
            probe_version: 1,
            probed_at: chrono::Utc::now().to_rfc3339(),
        };

        let profiles = crate::providers::ProviderProfileStore::new(app.storage.clone());
        profiles
            .create_with_models_and_activate_session(
                crate::storage::ProviderProfileInput {
                    profile_id: None,
                    owner_id: owner1.into(),
                    alias: "custom".into(),
                    endpoint: "https://test".into(),
                    protocol: "openai_chat_completions".into(),
                    credential_ref: None,
                    api_key_ref: None,
                    safe_headers_json: "{}".into(),
                    secret_headers_ref: None,
                },
                std::slice::from_ref(&model_rec),
                &session1.id,
                "m",
            )
            .unwrap();

        let p2 = tg.resolve_custom_alias(owner1, "custom").unwrap();
        assert_eq!(p2, "custom_1");

        // Add custom_2 to create a gap
        profiles
            .create_with_models_and_activate_session(
                crate::storage::ProviderProfileInput {
                    profile_id: None,
                    owner_id: owner1.into(),
                    alias: "custom_2".into(),
                    endpoint: "https://test".into(),
                    protocol: "openai_chat_completions".into(),
                    credential_ref: None,
                    api_key_ref: None,
                    safe_headers_json: "{}".into(),
                    secret_headers_ref: None,
                },
                &[model_rec],
                &session1.id,
                "m",
            )
            .unwrap();

        // Should fill the gap and return custom_1
        let p3 = tg.resolve_custom_alias(owner1, "custom").unwrap();
        assert_eq!(p3, "custom_1");

        // Other owner should still get "custom"
        let p4 = tg.resolve_custom_alias(owner2, "custom").unwrap();
        assert_eq!(p4, "custom");
    }

    use super::*;
    use crate::tools::{ToolContext, ToolEffect, ToolOrigin, ToolPolicy, ToolRisk, ToolSpec};
    use axum::{
        body::Bytes,
        extract::State,
        http::Uri,
        routing::{get, post},
        Json, Router,
    };
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tokio::net::TcpListener;

    #[derive(Default)]
    struct ProviderProbe {
        started: AtomicBool,
        natural_completion: AtomicBool,
    }

    #[derive(Default)]
    struct TelegramRequestProbe {
        requests: Mutex<Vec<(String, serde_json::Value)>>,
    }

    async fn scoped_telegram_stub(
        State(probe): State<Arc<TelegramRequestProbe>>,
        uri: Uri,
        body: Bytes,
    ) -> Json<serde_json::Value> {
        let method = uri.path().rsplit('/').next().unwrap_or_default();
        let value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        probe.requests.lock().unwrap().push((method.into(), value));
        let result = match method {
            "answerCallbackQuery" | "deleteMessage" | "sendRichMessageDraft" => json!(true),
            _ => json!({"message_id":77,"chat":{"id":100,"type":"supergroup"}}),
        };
        Json(json!({
            "ok":true,
            "result":result
        }))
    }

    async fn slow_provider(State(probe): State<Arc<ProviderProbe>>) -> Json<serde_json::Value> {
        probe.started.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(5)).await;
        probe.natural_completion.store(true, Ordering::SeqCst);
        Json(json!({"output":[{"content":[{"type":"output_text","text":"natural completion"}]}]}))
    }

    async fn telegram_stub(uri: Uri) -> Json<serde_json::Value> {
        let method = uri.path().rsplit('/').next().unwrap_or_default();
        let result = match method {
            "sendRichMessageDraft" | "answerCallbackQuery" | "deleteMessage" => json!(true),
            "sendRichMessage" | "sendMessage" | "editMessageText" | "editMessageReplyMarkup" => {
                json!({"message_id":77,"chat":{"id":100,"type":"private"}})
            }
            "getMe" => {
                json!({"id":1,"is_bot":true,"first_name":"xiao test","username":"xiao_test_bot"})
            }
            _ => json!(true),
        };
        Json(json!({"ok":true,"result":result}))
    }

    async fn serve(router: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{address}")
    }

    fn user(id: i64) -> types::User {
        types::User {
            id,
            is_bot: false,
            first_name: format!("u{id}"),
            username: None,
        }
    }
    fn message(update_id: i64, chat_id: i64, user_id: i64, text: &str) -> Update {
        Update {
            update_id,
            message: Some(Message {
                message_id: update_id,
                message_thread_id: None,
                chat: types::Chat {
                    id: chat_id,
                    kind: "private".into(),
                },
                from: Some(user(user_id)),
                text: Some(text.into()),
                caption: None,
                photo: Vec::new(),
                document: None,
            }),
            callback_query: None,
        }
    }

    fn topic_message(
        update_id: i64,
        chat_id: i64,
        thread_id: i64,
        user_id: i64,
        text: &str,
    ) -> Update {
        let mut update = message(update_id, chat_id, user_id, text);
        let message = update.message.as_mut().unwrap();
        message.message_thread_id = Some(thread_id);
        message.chat.kind = "supergroup".into();
        update
    }

    fn callback(
        update_id: i64,
        chat_id: i64,
        thread_id: i64,
        user_id: i64,
        data: String,
    ) -> Update {
        Update {
            update_id,
            message: None,
            callback_query: Some(CallbackQuery {
                id: format!("callback-{update_id}"),
                from: user(user_id),
                message: Some(Message {
                    message_id: 77,
                    message_thread_id: Some(thread_id),
                    chat: types::Chat {
                        id: chat_id,
                        kind: "supergroup".into(),
                    },
                    from: Some(user(user_id)),
                    text: None,
                    caption: None,
                    photo: Vec::new(),
                    document: None,
                }),
                data: Some(data),
            }),
        }
    }

    fn last_callbacks(probe: &TelegramRequestProbe) -> Vec<String> {
        let requests = probe.requests.lock().unwrap();
        let body = requests
            .iter()
            .rev()
            .find(|(method, body)| {
                matches!(
                    method.as_str(),
                    "sendRichMessage" | "sendMessage" | "editMessageText"
                ) && body.pointer("/reply_markup/inline_keyboard").is_some()
            })
            .map(|(_, body)| body)
            .expect("Telegram menu payload");
        body.pointer("/reply_markup/inline_keyboard")
            .and_then(serde_json::Value::as_array)
            .unwrap()
            .iter()
            .flat_map(|row| row.as_array().into_iter().flatten())
            .filter_map(|button| {
                button
                    .get("callback_data")
                    .and_then(serde_json::Value::as_str)
            })
            .map(str::to_owned)
            .collect()
    }

    fn progress_items(view: &View) -> &[ProgressItem] {
        match view.blocks.first() {
            Some(Block::Progress { items }) => items,
            _ => panic!("expected a progress block"),
        }
    }

    async fn wait_processed(storage: &crate::storage::Storage, id: i64) {
        tokio::time::timeout(Duration::from_secs(2),async {
            loop {
                if matches!(storage.telegram_update_status(id).unwrap(),Some((ref status,_)) if status=="processed") { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap_or_else(|_|panic!("Telegram update {id} was not processed promptly"));
    }

    #[tokio::test]
    async fn stop_preempts_pending_rename_and_custom_login_input() {
        let telegram_base = serve(Router::new().fallback(post(telegram_stub))).await;
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::AppConfig::default();
        cfg.storage.database = temp.path().join("xiao.db");
        cfg.paths.data_dir = temp.path().join("data");
        cfg.paths.logs_dir = temp.path().join("logs");
        cfg.paths.secrets_dir = temp.path().join("secrets");
        cfg.telegram.enabled = true;
        cfg.telegram.access.allowed_chat_ids = vec![100];
        cfg.telegram.access.owner_user_id = Some(10);
        let app = AppState::build(cfg).await.unwrap();
        let scope = TelegramScope::new(100, None);
        let menus = Arc::new(MenuStore::new(Duration::from_secs(60)));
        let menu = menus.prepare_scoped(scope, 10, View::info("RENAME", "pending"));
        let menu_id = menu.lock().await.id.clone();
        menus.insert(menu.clone(), menu_id.clone());
        let custom_logins = Arc::new(CustomLoginStore::new(Duration::from_secs(60)));
        let adapter = TelegramAdapter {
            app,
            client: TelegramClient::with_base("test-token".into(), telegram_base).unwrap(),
            menus,
            custom_logins: custom_logins.clone(),
            principal_locks: Arc::new(Mutex::new(HashMap::new())),
            active_work: Arc::new(Mutex::new(HashMap::new())),
        };

        menu.lock().await.pending_input = Some("/sessions rename session-a".into());
        adapter
            .handle_update(message(1, 100, 10, "/stop"))
            .await
            .unwrap();
        assert_eq!(
            menu.lock().await.pending_input.as_deref(),
            Some("/sessions rename session-a")
        );

        let wizard = custom_logins.begin(scope, 10, menu_id);
        let wizard_id = wizard.lock().await.id.clone();
        menu.lock().await.pending_input = Some(format!("custom:{wizard_id}:endpoint"));
        adapter
            .handle_update(message(2, 100, 10, "/stop@xiao_test_bot"))
            .await
            .unwrap();
        assert_eq!(
            menu.lock().await.pending_input.as_deref(),
            Some(format!("custom:{wizard_id}:endpoint").as_str())
        );
        assert!(custom_logins.get(&wizard_id).is_some());
    }

    #[test]
    fn stop_detection_accepts_only_the_canonical_command_head() {
        assert!(is_stop_command("/stop"));
        assert!(is_stop_command("  /stop@xiao_test_bot extra"));
        assert!(!is_stop_command("/s"));
        assert!(!is_stop_command("/stop-now"));
    }

    #[test]
    fn final_surface_excludes_progress_and_keeps_side_marker() {
        let view = agent_final_view(AgentAnswer {
            progress: vec![AgentEvent::ToolCompleted {
                tool: "shell".into(),
                summary: "SECRET HUGE TOOL OUTPUT".into(),
            }],
            final_answer: "clean answer".into(),
            side_mode: true,
            artifacts: Vec::new(),
        });
        let rendered = rich::render(&view, false).to_string();
        assert!(rendered.contains("clean answer"));
        assert!(rendered.contains("SIDE CHAT SESSION"));
        assert!(!rendered.contains("SECRET HUGE TOOL OUTPUT"));
        assert!(!rendered.contains("thinking"));
    }

    #[test]
    fn progress_is_bounded_and_redacted() {
        let mut p = ProgressAggregator::new("normal".into());
        p.push(AgentEvent::Status(
            "Authorization: very-secret-token".into(),
        ));
        let value = rich::render(&p.view(), true).to_string();
        assert!(!value.contains("very-secret-token"));
        assert!(value.contains("thinking"));
    }

    #[test]
    fn progress_maps_real_work_to_semantic_activities() {
        for (tool, activity, label) in [
            (
                "web_search",
                ProgressActivity::Searching,
                "Searching the web",
            ),
            ("web_fetch", ProgressActivity::Fetching, "Fetching a page"),
            (
                "context_stats",
                ProgressActivity::Analyzing,
                "Checking conversation context",
            ),
            (
                "code_interpreter",
                ProgressActivity::Coding,
                "Working with code",
            ),
            (
                "image_generation",
                ProgressActivity::Media,
                "Processing media",
            ),
        ] {
            let mut progress = ProgressAggregator::new("normal".into());
            progress.push(AgentEvent::ToolStarted(tool.into()));
            let view = progress.view();
            let item = progress_items(&view).last().unwrap();
            assert_eq!(item.state, ProgressState::Active);
            assert_eq!(item.activity, activity);
            assert_eq!(item.label, label);
        }
    }

    #[test]
    fn completed_tool_remains_visible_without_synthetic_thinking() {
        let mut progress = ProgressAggregator::new("normal".into());
        progress.push(AgentEvent::GenerationStarted);
        progress.push(AgentEvent::ToolStarted("web_search".into()));
        progress.push(AgentEvent::StreamChunk {
            provider: "codex".into(),
            bytes: 64,
        });
        progress.push(AgentEvent::ToolCompleted {
            tool: "web_search".into(),
            summary: "completed".into(),
        });
        let view = progress.view();
        let items = progress_items(&view);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].state, ProgressState::Done);
        assert_eq!(items[0].label, "Thinking");
        assert_eq!(items[1].state, ProgressState::Done);
        assert_eq!(items[1].label, "Searched the web");
    }

    #[test]
    fn stream_progress_updates_one_writing_step_in_place() {
        let mut progress = ProgressAggregator::new("detailed".into());
        progress.push(AgentEvent::GenerationStarted);
        for _ in 0..16 {
            progress.push(AgentEvent::StreamChunk {
                provider: "codex".into(),
                bytes: 64,
            });
        }
        let view = progress.view();
        let items = progress_items(&view);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].activity, ProgressActivity::Writing);
        assert_eq!(items[0].label, "Writing response · 16 chunks");
    }

    #[test]
    fn minimal_progress_keeps_the_completed_action_visible() {
        let mut progress = ProgressAggregator::new("minimal".into());
        progress.push(AgentEvent::GenerationStarted);
        progress.push(AgentEvent::ToolStarted("web_fetch".into()));
        let active = progress.view();
        assert_eq!(progress_items(&active).len(), 1);
        assert_eq!(
            progress_items(&active)[0].activity,
            ProgressActivity::Fetching
        );
        progress.push(AgentEvent::ToolCompleted {
            tool: "web_fetch".into(),
            summary: "completed".into(),
        });
        let resumed = progress.view();
        assert_eq!(progress_items(&resumed).len(), 1);
        assert_eq!(progress_items(&resumed)[0].state, ProgressState::Done);
    }

    #[test]
    fn late_tool_completion_never_completes_a_different_active_tool() {
        let mut progress = ProgressAggregator::new("normal".into());
        progress.push(AgentEvent::ToolStarted("web_search".into()));
        progress.push(AgentEvent::ToolStarted("code_interpreter".into()));
        progress.push(AgentEvent::ToolCompleted {
            tool: "web_search".into(),
            summary: "completed".into(),
        });

        let view = progress.view();
        let items = progress_items(&view);
        assert_eq!(
            items
                .iter()
                .filter(|item| item.state == ProgressState::Active)
                .count(),
            1
        );
        let active = items.last().unwrap();
        assert_eq!(active.activity, ProgressActivity::Coding);
        assert_eq!(active.label, "Working with code");
    }

    #[test]
    fn normal_timeline_retains_24_append_oriented_rows() {
        let mut progress = ProgressAggregator::new("normal".into());
        for index in 0..40 {
            progress.push(AgentEvent::ToolStartedWithId {
                tool: format!("operation_{index}"),
                call_id: format!("call-{index}"),
            });
        }
        let view = progress.view();
        let items = progress_items(&view);
        assert_eq!(items.len(), 24);
        assert!(items[..23]
            .iter()
            .all(|item| item.state == ProgressState::Done));
        assert_eq!(items.last().unwrap().state, ProgressState::Active);
        assert_eq!(
            items.last().unwrap().correlation_id.as_deref(),
            Some("call-39")
        );
    }

    #[test]
    fn detailed_timeline_retains_30_append_oriented_rows() {
        let mut progress = ProgressAggregator::new("detailed".into());
        for index in 0..40 {
            progress.push(AgentEvent::ToolStartedWithId {
                tool: format!("operation_{index}"),
                call_id: format!("call-{index}"),
            });
        }
        let view = progress.view();
        let items = progress_items(&view);
        assert_eq!(items.len(), 30);
        assert!(items[..29]
            .iter()
            .all(|item| item.state == ProgressState::Done));
        assert_eq!(items.last().unwrap().state, ProgressState::Active);
        assert_eq!(
            items.last().unwrap().correlation_id.as_deref(),
            Some("call-39")
        );
    }

    #[test]
    fn correlation_id_completes_exact_tool_row_and_rejects_wrong_id() {
        let mut progress = ProgressAggregator::new("normal".into());
        progress.push(AgentEvent::ToolStartedWithId {
            tool: "web_search".into(),
            call_id: "old-call".into(),
        });
        progress.push(AgentEvent::ToolStartedWithId {
            tool: "web_search".into(),
            call_id: "new-call".into(),
        });
        progress.push(AgentEvent::ToolCompletedWithId {
            tool: "web_search".into(),
            call_id: "wrong-call".into(),
            summary: "completed".into(),
        });
        let view = progress.view();
        let items = progress_items(&view);
        assert_eq!(items[0].state, ProgressState::Done);
        assert_eq!(items[1].state, ProgressState::Active);
        progress.push(AgentEvent::ToolCompletedWithId {
            tool: "web_search".into(),
            call_id: "old-call".into(),
            summary: "completed".into(),
        });
        progress.push(AgentEvent::ToolCompletedWithId {
            tool: "web_search".into(),
            call_id: "new-call".into(),
            summary: "completed".into(),
        });
        let view = progress.view();
        let items = progress_items(&view);
        assert!(items.iter().all(|item| item.state == ProgressState::Done));
        assert_eq!(items[0].correlation_id.as_deref(), Some("old-call"));
        assert_eq!(items[1].correlation_id.as_deref(), Some("new-call"));
    }

    #[test]
    fn failed_tool_stays_visible_with_redacted_error_and_failure_icon() {
        let mut progress = ProgressAggregator::new("detailed".into());
        progress.push(AgentEvent::ToolStartedWithId {
            tool: "terminal".into(),
            call_id: "terminal-1".into(),
        });
        progress.push(AgentEvent::ToolCompletedWithId {
            tool: "terminal".into(),
            call_id: "terminal-1".into(),
            summary: "failed: Authorization: very-secret-token".into(),
        });
        let view = progress.view();
        let items = progress_items(&view);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, ProgressState::Failed);
        let rendered = rich::render(&view, true).to_string();
        assert!(rendered.contains("✗"));
        assert!(!rendered.contains("very-secret-token"));
    }

    #[test]
    fn hard_progress_budget_preserves_active_and_recent_rows() {
        let mut progress = ProgressAggregator::new("detailed".into());
        for index in 0..30 {
            progress.push(AgentEvent::ToolStartedWithId {
                tool: format!("operation_{index}_{}", "x".repeat(180)),
                call_id: format!("call-{index}"),
            });
        }
        let view = progress.view();
        let items = progress_items(&view);
        assert!(progress_text_length(items) <= PROGRESS_CHAR_BUDGET);
        assert_eq!(items.last().unwrap().state, ProgressState::Active);
        assert_eq!(
            items.last().unwrap().correlation_id.as_deref(),
            Some("call-29")
        );
        assert!(items.len() >= 2);
        assert_eq!(
            items[items.len() - 2].correlation_id.as_deref(),
            Some("call-28")
        );
    }

    #[test]
    fn action_classifier_is_presentation_only_and_does_not_relax_policy() {
        let classified = tool_progress("termux_terminal");
        assert_eq!(classified.icon, ProgressIcon::Terminal);
        let spec = ToolSpec {
            name: "termux_terminal".into(),
            description: "test".into(),
            parameters: serde_json::json!({"type":"object"}),
            risk: ToolRisk::SideEffect,
            origin: ToolOrigin::Termux,
            effect: ToolEffect::NonIdempotent,
            required_capabilities: Vec::new(),
            timeout_ms: 1_000,
        };
        let context = ToolContext {
            principal: "owner".into(),
            session_id: "session".into(),
            agent_run_id: "run".into(),
            yolo_mode: false,
            messages: Vec::new(),
            cancellation: CancellationToken::new(),
            progress: None,
        };
        assert!(matches!(
            ToolPolicy::default().evaluate_call(
                &spec,
                &serde_json::json!({"program":"rm","args":["file"]}),
                &context,
            ),
            crate::tools::PolicyDecision::RequireApproval(_)
        ));
    }

    #[test]
    fn oversized_final_is_paginated_without_losing_content() {
        let source = "z".repeat(9000);
        let view = View::from_markdown(&source, false);
        let pages = paginate_final_view(&view, 3500);
        assert!(pages.len() >= 3);
        assert_eq!(pages.iter().map(rich::plain).collect::<String>(), source);
    }

    #[tokio::test]
    async fn long_generation_does_not_block_stop_other_principal_or_callbacks() {
        let probe = Arc::new(ProviderProbe::default());
        let provider_base = serve(
            Router::new()
                .route("/v1/responses", post(slow_provider))
                .with_state(probe.clone()),
        )
        .await;
        let telegram_base = serve(Router::new().fallback(post(telegram_stub))).await;

        let temp = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::AppConfig::default();
        cfg.storage.database = temp.path().join("xiao.db");
        cfg.paths.data_dir = temp.path().join("data");
        cfg.paths.logs_dir = temp.path().join("logs");
        cfg.paths.secrets_dir = temp.path().join("secrets");
        cfg.telegram.enabled = true;
        cfg.telegram.access.allowed_chat_ids = vec![100, 200];
        cfg.telegram.access.owner_user_id = Some(10);
        cfg.providers.codex.enabled = false;
        cfg.providers.antigravity.enabled = false;
        cfg.providers.custom.enabled = true;
        cfg.providers.custom.protocol = "openai_responses".into();
        cfg.providers.custom.tool_protocol = "native".into();
        cfg.providers.custom.base_url = Some(format!("{provider_base}/v1"));
        cfg.providers.custom.models = vec!["m".into()];
        cfg.providers.custom.default_model = Some("m".into());
        let app = AppState::build(cfg).await.unwrap();
        app.storage
            .upsert_provider_capability(&crate::storage::ProviderCapabilityRecord {
                provider: "custom".into(),
                model: "m".into(),
                tool_protocol: "native".into(),
                native_tool_calls: true,
                structured_output: false,
                continuation: true,
                probe_status: "completed".into(),
                probe_version: 1,
                probed_at: chrono::Utc::now().to_rfc3339(),
                evidence: "fixture isolates native agent cancellation from semantic evaluation"
                    .into(),
            })
            .unwrap();
        let principal_a = TelegramAdapter::principal(&app, 10);
        let main = app.sessions.ensure_default_session(&principal_a).unwrap();
        app.storage
            .set_session_provider(&principal_a, &main.id, "custom", None, "m")
            .unwrap();
        app.sessions.switch_main(&principal_a, &main.id).unwrap();

        let adapter = TelegramAdapter {
            app: app.clone(),
            client: TelegramClient::with_base("test-token".into(), telegram_base).unwrap(),
            menus: Arc::new(MenuStore::new(Duration::from_secs(60))),
            custom_logins: Arc::new(CustomLoginStore::new(Duration::from_secs(60))),
            principal_locks: Arc::new(Mutex::new(HashMap::new())),
            active_work: Arc::new(Mutex::new(HashMap::new())),
        };

        let first = message(1, 100, 10, "run a long request");
        assert!(app
            .storage
            .enqueue_telegram_update(1, &serde_json::to_string(&first).unwrap())
            .unwrap());
        adapter.spawn_update(first);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !probe.started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake provider never started");

        // Another allowed chat for the same single owner remains responsive while
        // generation is active; no second owner/principal exists in Xiao.
        let status = message(2, 200, 10, "/status");
        assert!(app
            .storage
            .enqueue_telegram_update(2, &serde_json::to_string(&status).unwrap())
            .unwrap());
        adapter.spawn_update(status);
        wait_processed(&app.storage, 2).await;
        assert!(!probe.natural_completion.load(Ordering::SeqCst));

        // A callback for principal A is acknowledged and routed while generation is active.
        let menu = adapter.menus.prepare(
            100,
            10,
            View {
                title: Some("TEST".into()),
                blocks: vec![],
                actions: vec![vec![Action::command("Status", "/status")]],
                side_mode: false,
            },
        );
        let (menu_id, revision) = {
            let mut guard = menu.lock().await;
            guard.message_id = 77;
            (guard.id.clone(), guard.revision)
        };
        adapter.menus.insert(menu, menu_id.clone());
        let callback = Update {
            update_id: 3,
            message: None,
            callback_query: Some(CallbackQuery {
                id: "cb-1".into(),
                from: user(10),
                message: Some(Message {
                    message_id: 77,
                    message_thread_id: None,
                    chat: types::Chat {
                        id: 100,
                        kind: "private".into(),
                    },
                    from: Some(user(10)),
                    text: None,
                    caption: None,
                    photo: Vec::new(),
                    document: None,
                }),
                data: Some(menu::callback_data(&menu_id, revision, 0)),
            }),
        };
        assert!(app
            .storage
            .enqueue_telegram_update(3, &serde_json::to_string(&callback).unwrap())
            .unwrap());
        adapter.spawn_update(callback);
        wait_processed(&app.storage, 3).await;
        assert!(!probe.natural_completion.load(Ordering::SeqCst));

        // /stop takes the fast semantic path in its own accepted update and cancels the
        // provider future before the fake provider's five-second natural completion.
        let stop = message(4, 100, 10, "/stop");
        assert!(app
            .storage
            .enqueue_telegram_update(4, &serde_json::to_string(&stop).unwrap())
            .unwrap());
        let stop_started = std::time::Instant::now();
        adapter.spawn_update(stop);
        wait_processed(&app.storage, 4).await;
        wait_processed(&app.storage, 1).await;
        assert!(stop_started.elapsed() < Duration::from_secs(2));
        assert!(!probe.natural_completion.load(Ordering::SeqCst));
        let messages = app.storage.messages(&principal_a, &main.id).unwrap();
        assert_eq!(messages.iter().filter(|m| m.role == "assistant").count(), 0);
    }

    #[tokio::test]
    async fn first_command_in_each_topic_creates_isolated_sessions_and_replies_in_topic() {
        let telegram_probe = Arc::new(TelegramRequestProbe::default());
        let telegram_base = serve(
            Router::new()
                .fallback(post(scoped_telegram_stub))
                .with_state(telegram_probe.clone()),
        )
        .await;
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::AppConfig::default();
        cfg.storage.database = temp.path().join("xiao.db");
        cfg.paths.data_dir = temp.path().join("data");
        cfg.paths.logs_dir = temp.path().join("logs");
        cfg.paths.secrets_dir = temp.path().join("secrets");
        cfg.telegram.enabled = true;
        cfg.telegram.access.allowed_chat_ids = vec![100];
        cfg.telegram.access.allowed_user_ids = vec![10];
        let app = AppState::build(cfg).await.unwrap();
        let adapter = TelegramAdapter {
            app: app.clone(),
            client: TelegramClient::with_base("test-token".into(), telegram_base).unwrap(),
            menus: Arc::new(MenuStore::new(Duration::from_secs(60))),
            custom_logins: Arc::new(CustomLoginStore::new(Duration::from_secs(60))),
            principal_locks: Arc::new(Mutex::new(HashMap::new())),
            active_work: Arc::new(Mutex::new(HashMap::new())),
        };
        adapter
            .handle_update(topic_message(1, 100, 10, 10, "/status"))
            .await
            .unwrap();
        adapter
            .handle_update(topic_message(2, 100, 20, 10, "/status"))
            .await
            .unwrap();

        let principal = TelegramAdapter::principal(&app, 10);
        let topic_10 = app
            .sessions
            .context_for_telegram(&principal, TelegramScope::new(100, Some(10)))
            .unwrap();
        let topic_20 = app
            .sessions
            .context_for_telegram(&principal, TelegramScope::new(100, Some(20)))
            .unwrap();
        assert_ne!(topic_10.main.id, topic_20.main.id);
        assert_eq!(
            app.sessions
                .list_telegram_page(&principal, TelegramScope::new(100, Some(10)), 1, 5)
                .unwrap()
                .0
                .len(),
            1
        );
        let requests = telegram_probe.requests.lock().unwrap();
        let threads = requests
            .iter()
            .filter(|(method, _)| matches!(method.as_str(), "sendRichMessage" | "sendMessage"))
            .map(|(_, body)| body["message_thread_id"].as_i64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(threads, [10, 20]);
    }

    #[tokio::test]
    async fn custom_login_wizard_discovers_pages_probes_and_rejects_wrong_topic_callbacks() {
        let provider = Router::new()
            .route(
                "/v1/models",
                get(|| async {
                    Json(json!({
                        "data":(0..12).map(|index| json!({"id":format!("model-{index:02}")})).collect::<Vec<_>>()
                    }))
                }),
            )
            .route(
                "/v1/chat/completions",
                post(|Json(body): Json<serde_json::Value>| async move {
                    // Hidden vision challenge: extract from image_url fragment (#VISION-...), not from text prompt.
                    let body_str = serde_json::to_string(&body).unwrap();
                    let nonce = body_str
                        .split("VISION-")
                        .nth(1)
                        .and_then(|tail| tail.split(['"', '#', '\'', ' ', '}']).next())
                        .unwrap_or("")
                        .split(['\\', '"'])
                        .next()
                        .unwrap()
                        .trim();
                    // Fallback to legacy text extraction for compatibility (should not happen after P0-2).
                    let fallback = body["messages"][0]["content"].as_str().unwrap_or("").to_string();
                    let challenge = if !nonce.is_empty() {
                        format!("VISION-{nonce}")
                    } else if let Some(n) = fallback.split("nonce ").nth(1).and_then(|s| s.split('.').next()) {
                        n.to_string()
                    } else {
                        "VISION-missing".to_string()
                    };
                    Json(json!({
                        "choices":[{"message":{"role":"assistant","content":challenge.clone(),"tool_calls":[{
                            "id":"probe","type":"function","function":{
                                "name":"xiao_capability_probe",
                                "arguments":json!({"nonce":challenge.clone()}).to_string()
                            }
                        }]}}]
                    }))
                }),
            );
        let provider_base = serve(provider).await;
        let telegram_probe = Arc::new(TelegramRequestProbe::default());
        let telegram_base = serve(
            Router::new()
                .fallback(post(scoped_telegram_stub))
                .with_state(telegram_probe.clone()),
        )
        .await;
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        let mut cfg = crate::config::AppConfig::default();
        cfg.storage.database = temp.path().join("xiao.db");
        cfg.paths.data_dir = temp.path().join("data");
        cfg.paths.logs_dir = temp.path().join("logs");
        cfg.paths.secrets_dir = temp.path().join("secrets");
        cfg.telegram.enabled = true;
        cfg.telegram.access.allowed_chat_ids = vec![100];
        cfg.telegram.access.owner_user_id = Some(10);
        cfg.save_atomic(&config_path).unwrap();
        let app = AppState::build_from_path(cfg, &config_path).await.unwrap();
        let adapter = TelegramAdapter {
            app: app.clone(),
            client: TelegramClient::with_base("test-token".into(), telegram_base).unwrap(),
            menus: Arc::new(MenuStore::new(Duration::from_secs(60))),
            custom_logins: Arc::new(CustomLoginStore::new(Duration::from_secs(60))),
            principal_locks: Arc::new(Mutex::new(HashMap::new())),
            active_work: Arc::new(Mutex::new(HashMap::new())),
        };

        adapter
            .handle_update(topic_message(1, 100, 10, 10, "/login"))
            .await
            .unwrap();
        adapter
            .handle_update(topic_message(
                2,
                100,
                10,
                10,
                &format!("{provider_base}/v1"),
            ))
            .await
            .unwrap();

        let skip = last_callbacks(&telegram_probe)[0].clone();
        // A non-owner cannot mutate this menu or its custom-login state.
        adapter
            .handle_update(callback(3, 100, 10, 11, skip.clone()))
            .await
            .unwrap();
        // Same owner/chat but wrong topic cannot mutate the wizard/menu.
        adapter
            .handle_update(callback(4, 100, 20, 10, skip.clone()))
            .await
            .unwrap();
        adapter
            .handle_update(callback(5, 100, 10, 10, skip))
            .await
            .unwrap();
        let default_alias = last_callbacks(&telegram_probe)[0].clone();
        adapter
            .handle_update(callback(6, 100, 10, 10, default_alias))
            .await
            .unwrap();

        let model_page_one = last_callbacks(&telegram_probe);
        assert_eq!(model_page_one.len(), 10);
        let next = model_page_one[7].clone();
        adapter
            .handle_update(callback(7, 100, 10, 10, next))
            .await
            .unwrap();
        let select_page_two_first = last_callbacks(&telegram_probe)[0].clone();
        adapter
            .handle_update(callback(8, 100, 10, 10, select_page_two_first))
            .await
            .unwrap();
        let confirm = last_callbacks(&telegram_probe)[0].clone();
        adapter
            .handle_update(callback(9, 100, 10, 10, confirm))
            .await
            .unwrap();

        let principal = TelegramAdapter::principal(&app, 10);
        let session = app
            .sessions
            .context_for_telegram(&principal, TelegramScope::new(100, Some(10)))
            .unwrap()
            .active;
        assert_eq!(session.provider, "custom");
        assert_eq!(session.model, "model-05");
        let profile_id = session
            .account_id
            .clone()
            .expect("Custom session profile selection");
        let profile_store = crate::providers::ProviderProfileStore::new(app.storage.clone());
        let capability = profile_store
            .model(&profile_id, "model-05")
            .unwrap()
            .unwrap();
        assert_eq!(capability.tool_protocol, "native");
        assert!(capability.native_tools);
        assert_eq!(
            app.providers.state("custom"),
            crate::providers::ProviderState::Ready
        );
        let saved = crate::config::AppConfig::load(&config_path).unwrap();
        assert!(saved.providers.custom.default_model.is_none());
        let profiles = profile_store.list(&principal).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].alias, "custom");
        assert_eq!(profiles[0].profile_id, profile_id);

        let requests = telegram_probe.requests.lock().unwrap();
        let prompt_indexes = requests
            .iter()
            .enumerate()
            .filter(|(_, (method, _))| matches!(method.as_str(), "sendRichMessage" | "sendMessage"))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert!(
            prompt_indexes.len() >= 5,
            "wizard stages must use new prompt messages"
        );
        for pair in prompt_indexes.windows(2).skip(1) {
            assert!(
                requests[pair[0]..pair[1]]
                    .iter()
                    .any(|(method, body)| method == "editMessageReplyMarkup"
                        && body.pointer("/reply_markup/inline_keyboard") == Some(&json!([]))),
                "the previous wizard keyboard must be retired before the next prompt"
            );
        }
    }

    #[tokio::test]
    async fn custom_wizard_retry_and_back_are_phase_aware_and_replace_transient_keys() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::AppConfig::default();
        cfg.storage.database = temp.path().join("xiao.db");
        cfg.paths.data_dir = temp.path().join("data");
        cfg.paths.logs_dir = temp.path().join("logs");
        cfg.paths.secrets_dir = temp.path().join("secrets");
        let app = AppState::build(cfg).await.unwrap();
        let scope = TelegramScope::new(100, Some(10));
        let menus = Arc::new(MenuStore::new(Duration::from_secs(60)));
        let menu = menus.prepare_scoped(scope, 10, View::info("LOGIN", "test"));
        let menu_id = menu.lock().await.id.clone();
        menus.insert(menu.clone(), menu_id.clone());
        let custom_logins = Arc::new(CustomLoginStore::new(Duration::from_secs(60)));
        let wizard = custom_logins.begin(scope, 10, menu_id);
        let wizard_id = wizard.lock().await.id.clone();
        let adapter = TelegramAdapter {
            app: app.clone(),
            client: TelegramClient::with_base("test-token".into(), "http://127.0.0.1:9".into())
                .unwrap(),
            menus,
            custom_logins,
            principal_locks: Arc::new(Mutex::new(HashMap::new())),
            active_work: Arc::new(Mutex::new(HashMap::new())),
        };
        let principal = app.resolve_telegram_owner(10).unwrap().owner_id;

        {
            let mut guard = menu.lock().await;
            adapter
                .handle_custom_action(
                    &mut guard,
                    &principal,
                    &format!("/_custom:{wizard_id}:retry"),
                )
                .await
                .unwrap();
            assert_eq!(
                guard.pending_input.as_deref(),
                Some(format!("custom:{wizard_id}:endpoint").as_str())
            );
            assert_eq!(
                guard.current_view.title.as_deref(),
                Some("CUSTOM LOGIN · ENDPOINT")
            );
        }

        {
            let mut state = wizard.lock().await;
            state.endpoint = Some("https://provider.example/v1".into());
            state.phase = CustomLoginPhase::ApiKey;
        }
        {
            let mut guard = menu.lock().await;
            adapter
                .handle_custom_action(
                    &mut guard,
                    &principal,
                    &format!("/_custom:{wizard_id}:retry"),
                )
                .await
                .unwrap();
            assert_eq!(
                guard.pending_input.as_deref(),
                Some(format!("custom:{wizard_id}:api_key").as_str())
            );
            assert_eq!(
                guard.current_view.title.as_deref(),
                Some("CUSTOM LOGIN · API KEY")
            );
        }

        let first = app
            .auth
            .create_api_key_credential("custom", "wizard-old", "OLD_KEY_SENTINEL")
            .unwrap();
        app.storage
            .set_account_owner(&principal, &first.id)
            .unwrap();
        {
            let mut state = wizard.lock().await;
            state.credential_ref = Some(first.id.clone());
            state.phase = CustomLoginPhase::Alias;
        }
        {
            let mut guard = menu.lock().await;
            adapter
                .handle_custom_action(
                    &mut guard,
                    &principal,
                    &format!("/_custom:{wizard_id}:wizard_back"),
                )
                .await
                .unwrap();
            assert_eq!(wizard.lock().await.phase, CustomLoginPhase::ApiKey);
            adapter
                .handle_custom_input(
                    &mut guard,
                    &format!("custom:{wizard_id}:api_key"),
                    "NEW_KEY_SENTINEL",
                    CustomInputContext {
                        scope,
                        user_id: 10,
                        update_id: 99,
                        message_id: 99,
                        principal: &principal,
                    },
                )
                .await
                .unwrap();
        }
        assert!(app.auth.credential(&first.id).unwrap().is_none());
        let replacement = wizard
            .lock()
            .await
            .credential_ref
            .clone()
            .expect("replacement credential reference");
        assert_ne!(replacement, first.id);
        assert_eq!(
            app.auth
                .credential(&replacement)
                .unwrap()
                .unwrap()
                .api_key
                .as_deref(),
            Some("NEW_KEY_SENTINEL")
        );

        {
            let mut state = wizard.lock().await;
            state.phase = CustomLoginPhase::ApiKey;
        }
        {
            let mut guard = menu.lock().await;
            adapter
                .handle_custom_action(
                    &mut guard,
                    &principal,
                    &format!("/_custom:{wizard_id}:skip_key"),
                )
                .await
                .unwrap();
        }
        assert!(app.auth.credential(&replacement).unwrap().is_none());
        assert!(wizard.lock().await.credential_ref.is_none());
    }

    #[tokio::test]
    async fn failed_custom_wizard_commit_restores_session_and_removes_partial_profile() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::AppConfig::default();
        cfg.storage.database = temp.path().join("xiao.db");
        cfg.paths.data_dir = temp.path().join("data");
        cfg.paths.logs_dir = temp.path().join("logs");
        cfg.paths.secrets_dir = temp.path().join("secrets");
        let app = AppState::build(cfg).await.unwrap();
        let scope = TelegramScope::new(100, Some(10));
        let principal = app.resolve_telegram_owner(10).unwrap().owner_id;
        let before = app
            .sessions
            .context_for_telegram(&principal, scope)
            .unwrap()
            .active;
        app.storage
            .with_conn(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER reject_custom_commit BEFORE INSERT ON audit_events
                     WHEN NEW.action='custom_provider_configured'
                     BEGIN SELECT RAISE(FAIL,'synthetic audit failure'); END;",
                )?;
                Ok(())
            })
            .unwrap();
        let menus = Arc::new(MenuStore::new(Duration::from_secs(60)));
        let menu = menus.prepare_scoped(scope, 10, View::info("LOGIN", "rollback"));
        let menu_id = menu.lock().await.id.clone();
        menus.insert(menu.clone(), menu_id.clone());
        let custom_logins = Arc::new(CustomLoginStore::new(Duration::from_secs(60)));
        let wizard = custom_logins.begin(scope, 10, menu_id);
        let wizard_id = {
            let mut state = wizard.lock().await;
            state.phase = CustomLoginPhase::Confirm;
            state.endpoint = Some("https://rollback.example/v1".into());
            state.protocol = "openai_chat_completions".into();
            state.alias = "rollback-profile".into();
            state.models = vec!["model-a".into()];
            state.selected_index = Some(0);
            state.capability = Some(crate::providers::CustomCapabilityProbe {
                capabilities: crate::providers::ProviderCapabilities::native("rollback fixture"),
                native_tools: crate::providers::CapabilityState::Supported,
                structured_output: crate::providers::CapabilityState::Supported,
                continuation: crate::providers::CapabilityState::Supported,
                vision: crate::providers::CapabilityState::Unsupported,
                file_input: crate::providers::CapabilityState::Unsupported,
            });
            state.id.clone()
        };
        let adapter = TelegramAdapter {
            app: app.clone(),
            client: TelegramClient::with_base("test-token".into(), "http://127.0.0.1:9".into())
                .unwrap(),
            menus,
            custom_logins: custom_logins.clone(),
            principal_locks: Arc::new(Mutex::new(HashMap::new())),
            active_work: Arc::new(Mutex::new(HashMap::new())),
        };
        {
            let mut guard = menu.lock().await;
            assert!(adapter
                .handle_custom_action(
                    &mut guard,
                    &principal,
                    &format!("/_custom:{wizard_id}:confirm"),
                )
                .await
                .is_err());
        }
        let after = app
            .storage
            .session(&principal, &before.id)
            .unwrap()
            .unwrap();
        assert_eq!(after.provider, before.provider);
        assert_eq!(after.account_id, before.account_id);
        assert_eq!(after.model, before.model);
        assert!(
            crate::providers::ProviderProfileStore::new(app.storage.clone())
                .list(&principal)
                .unwrap()
                .is_empty()
        );

        app.storage
            .with_conn(|connection| {
                connection.execute_batch("DROP TRIGGER reject_custom_commit;")?;
                Ok(())
            })
            .unwrap();
        {
            let mut guard = menu.lock().await;
            adapter
                .handle_custom_action(
                    &mut guard,
                    &principal,
                    &format!("/_custom:{wizard_id}:retry"),
                )
                .await
                .unwrap();
            assert_eq!(guard.current_view.title.as_deref(), Some("CUSTOM LOGIN"));
        }
        assert!(custom_logins.get(&wizard_id).is_none());
        let selected = app
            .storage
            .session(&principal, &before.id)
            .unwrap()
            .unwrap();
        assert_eq!(selected.provider, "custom");
        assert_eq!(selected.model, "model-a");
        assert_eq!(
            crate::providers::ProviderProfileStore::new(app.storage.clone())
                .list(&principal)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn telegram_photo_and_document_are_downloaded_scoped_and_indexed() {
        async fn get_file(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
            let file_id = body["file_id"].as_str().unwrap_or_default();
            let (unique, path, size) = if file_id == "photo-file" {
                ("photo-unique", "files/photo.png", 68)
            } else {
                ("document-unique", "files/note.txt", 53)
            };
            Json(json!({"ok":true,"result":{
                "file_id":file_id,"file_unique_id":unique,"file_size":size,"file_path":path
            }}))
        }
        async fn photo_download() -> Vec<u8> {
            vec![
                137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0,
                1, 8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100,
                248, 15, 0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
                130,
            ]
        }
        async fn document_download() -> &'static str {
            "Telegram document sentinel content for indexed retrieval."
        }

        let telegram_probe = Arc::new(TelegramRequestProbe::default());
        let telegram_base = serve(
            Router::new()
                .route("/bottest-token/getFile", post(get_file))
                .route("/file/bottest-token/files/photo.png", get(photo_download))
                .route("/file/bottest-token/files/note.txt", get(document_download))
                .fallback(post(scoped_telegram_stub))
                .with_state(telegram_probe),
        )
        .await;
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::AppConfig::default();
        cfg.storage.database = temp.path().join("xiao.db");
        cfg.paths.data_dir = temp.path().join("data");
        cfg.paths.logs_dir = temp.path().join("logs");
        cfg.paths.secrets_dir = temp.path().join("secrets");
        cfg.telegram.enabled = true;
        cfg.telegram.access.owner_user_id = Some(10);
        cfg.telegram.access.allowed_chat_ids = vec![100];
        let app = AppState::build(cfg).await.unwrap();
        let adapter = TelegramAdapter {
            app: app.clone(),
            client: TelegramClient::with_base("test-token".into(), telegram_base).unwrap(),
            menus: Arc::new(MenuStore::new(Duration::from_secs(60))),
            custom_logins: Arc::new(CustomLoginStore::new(Duration::from_secs(60))),
            principal_locks: Arc::new(Mutex::new(HashMap::new())),
            active_work: Arc::new(Mutex::new(HashMap::new())),
        };

        let mut document = topic_message(70, 100, 7, 10, "");
        let message = document.message.as_mut().unwrap();
        message.text = None;
        message.caption = Some("Summarize this document".into());
        message.document = Some(types::Document {
            file_id: "document-file".into(),
            file_unique_id: "document-unique".into(),
            file_name: Some("note.txt".into()),
            mime_type: Some("text/plain".into()),
            file_size: Some(53),
        });
        adapter.handle_update(document).await.unwrap();

        let mut photo = topic_message(71, 100, 7, 10, "");
        let message = photo.message.as_mut().unwrap();
        message.text = None;
        message.caption = Some("What is in this image?".into());
        message.photo = vec![types::PhotoSize {
            file_id: "photo-file".into(),
            file_unique_id: "photo-unique".into(),
            width: 1,
            height: 1,
            file_size: Some(68),
        }];
        adapter.handle_update(photo).await.unwrap();

        let owner = app.resolve_telegram_owner(10).unwrap().owner_id;
        let session = app
            .sessions
            .context_for_telegram(&owner, TelegramScope::new(100, Some(7)))
            .unwrap()
            .active;
        let records = app
            .storage
            .recent_attachments(&owner, &session.id, 10)
            .unwrap();
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|record| record.kind == "image"));
        assert!(records
            .iter()
            .any(|record| { record.kind == "document" && record.processing_status == "ready" }));
        assert!(!app
            .storage
            .search_attachment_chunks(&owner, &session.id, "sentinel retrieval", 2)
            .unwrap()
            .is_empty());
    }
}
