use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::Local;

use crate::storage::{MessageRecord, SessionDeletionResult, SessionRecord, Storage};
use crate::telegram::TelegramScope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatMode {
    Main,
    Side,
}
impl ChatMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Side => "side",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionContext {
    pub main: SessionRecord,
    pub active: SessionRecord,
    pub mode: ChatMode,
}

pub struct SessionManager {
    storage: Arc<Storage>,
}
impl SessionManager {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn ensure_default_session(&self, principal: &str) -> Result<SessionRecord> {
        if let Some(s) = self
            .storage
            .list_main_sessions(principal, 1, 0, false)?
            .into_iter()
            .next()
        {
            return Ok(s);
        }
        self.new_main(principal)
    }

    pub fn new_main(&self, principal: &str) -> Result<SessionRecord> {
        let name = format!("Session {}", Local::now().format("%d %b %H:%M"));
        let mut provider = "custom".to_string();
        let mut account_id = None;
        let mut model = "default".to_string();

        if let Ok(Some((main_id, _, _))) = self.storage.frontend_state(principal) {
            if let Ok(Some(s)) = self.storage.session(principal, &main_id) {
                provider = s.provider;
                account_id = s.account_id;
                model = s.model;
            }
        }

        self.storage.create_session(
            principal,
            &name,
            &provider,
            account_id.as_deref(),
            &model,
            false,
            None,
        )
    }

    pub fn context_for(&self, principal: &str) -> Result<SessionContext> {
        let default = self.ensure_default_session(principal)?;
        let (main_id, side_id, mode) = match self.storage.frontend_state(principal)? {
            Some(v) => v,
            None => {
                self.storage
                    .set_frontend_state(principal, &default.id, None, "main")?;
                (default.id, None, "main".into())
            }
        };
        let main = match self.storage.session(principal, &main_id)? {
            Some(s) if !s.archived && !s.is_side => s,
            _ => {
                let fallback = self.ensure_default_session(principal)?;
                self.storage
                    .set_frontend_state(principal, &fallback.id, None, "main")?;
                fallback
            }
        };
        if mode == "side" {
            if let Some(sid) = side_id {
                if let Some(active) = self.storage.session(principal, &sid)? {
                    if active.is_side
                        && !active.archived
                        && active.parent_id.as_deref() == Some(main.id.as_str())
                    {
                        return Ok(SessionContext {
                            main,
                            active,
                            mode: ChatMode::Side,
                        });
                    }
                }
            }
            self.storage
                .set_frontend_state(principal, &main.id, None, "main")?;
        }
        Ok(SessionContext {
            main: main.clone(),
            active: main,
            mode: ChatMode::Main,
        })
    }

    /// Resolve one exact non-archived session without reading or mutating any
    /// frontend active-session pointer. This is used by CLI `--session` so a
    /// terminal request can never silently inherit Telegram state.
    pub fn context_for_session(&self, principal: &str, id: &str) -> Result<SessionContext> {
        let active = self
            .storage
            .session(principal, id)?
            .ok_or_else(|| anyhow!("session not found"))?;
        if active.archived {
            return Err(anyhow!("session is archived"));
        }
        if active.is_side {
            let parent_id = active
                .parent_id
                .as_deref()
                .ok_or_else(|| anyhow!("side session is missing its parent"))?;
            let main = self
                .storage
                .session(principal, parent_id)?
                .ok_or_else(|| anyhow!("side session parent not found"))?;
            return Ok(SessionContext {
                main,
                active,
                mode: ChatMode::Side,
            });
        }
        Ok(SessionContext {
            main: active.clone(),
            active,
            mode: ChatMode::Main,
        })
    }

    pub fn append_user_to_session(
        &self,
        principal: &str,
        session_id: &str,
        text: &str,
    ) -> Result<SessionContext> {
        let context = self.context_for_session(principal, session_id)?;
        self.storage
            .append_message(principal, &context.active.id, "user", text)?;
        Ok(context)
    }

    pub fn switch_main(&self, principal: &str, id: &str) -> Result<SessionRecord> {
        let s = self
            .storage
            .session(principal, id)?
            .ok_or_else(|| anyhow!("session not found"))?;
        if s.is_side || s.archived {
            return Err(anyhow!("session is not selectable"));
        }
        self.storage
            .set_frontend_state(principal, &s.id, None, "main")?;
        Ok(s)
    }

    pub fn create_and_switch(&self, principal: &str) -> Result<SessionRecord> {
        let s = self.new_main(principal)?;
        self.storage
            .set_frontend_state(principal, &s.id, None, "main")?;
        Ok(s)
    }

    pub fn archive_and_recover(&self, principal: &str, id: &str) -> Result<SessionRecord> {
        let target = self
            .storage
            .session(principal, id)?
            .ok_or_else(|| anyhow!("session not found"))?;
        if target.is_side {
            return Err(anyhow!(
                "side sessions cannot be archived from the main manager"
            ));
        }
        self.storage.archive_session(principal, id)?;
        let context = self.context_for(principal).ok();
        if let Some(c) = context {
            if c.main.id != id && !c.main.archived {
                return Ok(c.main);
            }
        }
        let fallback = self
            .storage
            .list_main_sessions(principal, 1, 0, false)?
            .into_iter()
            .next()
            .unwrap_or(self.new_main(principal)?);
        self.storage
            .set_frontend_state(principal, &fallback.id, None, "main")?;
        Ok(fallback)
    }

    pub fn toggle_side(&self, principal: &str) -> Result<SessionContext> {
        let c = self.context_for(principal)?;
        match c.mode {
            ChatMode::Main => {
                let side = self.storage.create_session(
                    principal,
                    &format!("Side · {}", c.main.name),
                    &c.main.provider,
                    c.main.account_id.as_deref(),
                    &c.main.model,
                    true,
                    Some(&c.main.id),
                )?;
                self.storage
                    .set_frontend_state(principal, &c.main.id, Some(&side.id), "side")?;
            }
            ChatMode::Side => self
                .storage
                .set_frontend_state(principal, &c.main.id, None, "main")?,
        }
        self.context_for(principal)
    }

    pub fn append_user(&self, principal: &str, text: &str) -> Result<SessionContext> {
        let c = self.context_for(principal)?;
        self.storage
            .append_message(principal, &c.active.id, "user", text)?;
        Ok(c)
    }

    pub fn append_assistant_to(&self, principal: &str, session_id: &str, text: &str) -> Result<()> {
        self.storage
            .append_message(principal, session_id, "assistant", text)
    }

    pub fn agent_context(&self, principal: &str) -> Result<Vec<MessageRecord>> {
        let c = self.context_for(principal)?;
        let mut out = self.storage.messages(principal, &c.main.id)?;
        if c.mode == ChatMode::Side {
            out.extend(self.storage.messages(principal, &c.active.id)?);
        }
        // Compatibility accessor for views/tests. Agent transmission is now
        // bounded by ContextEngine using a character budget, not a row count.
        Ok(out)
    }

    pub fn list_page(
        &self,
        principal: &str,
        page: usize,
        page_size: usize,
    ) -> Result<(Vec<SessionRecord>, usize, usize)> {
        let page_size = page_size.max(1);
        let total = self.storage.count_main_sessions(principal)?;
        let pages = total.max(1).div_ceil(page_size).max(1);
        let p = page.clamp(1, pages);
        Ok((
            self.storage
                .list_main_sessions(principal, page_size, (p - 1) * page_size, false)?,
            pages,
            p,
        ))
    }

    pub fn ensure_telegram_session(
        &self,
        principal: &str,
        scope: TelegramScope,
    ) -> Result<SessionRecord> {
        if let Some(session) = self
            .storage
            .list_main_sessions_in_telegram_scope(principal, scope, 1, 0, false)?
            .into_iter()
            .next()
        {
            return Ok(session);
        }
        self.new_telegram_main(principal, scope)
    }

    pub fn new_telegram_main(
        &self,
        principal: &str,
        scope: TelegramScope,
    ) -> Result<SessionRecord> {
        let name = format!("Session {}", Local::now().format("%d %b %H:%M"));
        let mut provider = "custom".to_string();
        let mut account_id = None;
        let mut model = "default".to_string();

        if let Ok(Some((main_id, _, _))) = self.storage.telegram_frontend_state(principal, scope) {
            if let Ok(Some(s)) = self.storage.session(principal, &main_id) {
                provider = s.provider;
                account_id = s.account_id;
                model = s.model;
            }
        }

        let session = self.storage.create_session(
            principal,
            &name,
            &provider,
            account_id.as_deref(),
            &model,
            false,
            None,
        )?;
        self.storage
            .bind_session_to_telegram_scope(principal, &session.id, scope)?;
        Ok(session)
    }

    pub fn context_for_telegram(
        &self,
        principal: &str,
        scope: TelegramScope,
    ) -> Result<SessionContext> {
        let default = self.ensure_telegram_session(principal, scope)?;
        let (main_id, side_id, mode) =
            match self.storage.telegram_frontend_state(principal, scope)? {
                Some(value) => value,
                None => {
                    self.storage.set_telegram_frontend_state(
                        principal,
                        scope,
                        &default.id,
                        None,
                        "main",
                    )?;
                    (default.id, None, "main".into())
                }
            };
        let main = match self.storage.session(principal, &main_id)? {
            Some(session)
                if !session.archived
                    && !session.is_side
                    && self.session_in_scope(principal, &session.id, scope)? =>
            {
                session
            }
            _ => {
                let fallback = self.ensure_telegram_session(principal, scope)?;
                self.storage.set_telegram_frontend_state(
                    principal,
                    scope,
                    &fallback.id,
                    None,
                    "main",
                )?;
                fallback
            }
        };
        if mode == "side" {
            if let Some(side_id) = side_id {
                if let Some(active) = self.storage.session(principal, &side_id)? {
                    if active.is_side
                        && !active.archived
                        && active.parent_id.as_deref() == Some(main.id.as_str())
                        && self.session_in_scope(principal, &active.id, scope)?
                    {
                        return Ok(SessionContext {
                            main,
                            active,
                            mode: ChatMode::Side,
                        });
                    }
                }
            }
            self.storage
                .set_telegram_frontend_state(principal, scope, &main.id, None, "main")?;
        }
        Ok(SessionContext {
            main: main.clone(),
            active: main,
            mode: ChatMode::Main,
        })
    }

    pub fn append_user_telegram(
        &self,
        principal: &str,
        scope: TelegramScope,
        text: &str,
    ) -> Result<SessionContext> {
        let context = self.context_for_telegram(principal, scope)?;
        self.storage
            .append_message(principal, &context.active.id, "user", text)?;
        Ok(context)
    }

    pub fn create_and_switch_telegram(
        &self,
        principal: &str,
        scope: TelegramScope,
    ) -> Result<SessionRecord> {
        let session = self.new_telegram_main(principal, scope)?;
        self.storage
            .set_telegram_frontend_state(principal, scope, &session.id, None, "main")?;
        Ok(session)
    }

    pub fn switch_telegram_main(
        &self,
        principal: &str,
        scope: TelegramScope,
        id: &str,
    ) -> Result<SessionRecord> {
        let session = self
            .storage
            .session(principal, id)?
            .ok_or_else(|| anyhow!("session not found"))?;
        if session.is_side || session.archived || !self.session_in_scope(principal, id, scope)? {
            return Err(anyhow!("session is not selectable in this Telegram topic"));
        }
        self.storage
            .set_telegram_frontend_state(principal, scope, id, None, "main")?;
        Ok(session)
    }

    pub fn toggle_telegram_side(
        &self,
        principal: &str,
        scope: TelegramScope,
    ) -> Result<SessionContext> {
        let context = self.context_for_telegram(principal, scope)?;
        match context.mode {
            ChatMode::Main => {
                let side = self.storage.create_session(
                    principal,
                    &format!("Side · {}", context.main.name),
                    &context.main.provider,
                    context.main.account_id.as_deref(),
                    &context.main.model,
                    true,
                    Some(&context.main.id),
                )?;
                self.storage
                    .bind_session_to_telegram_scope(principal, &side.id, scope)?;
                self.storage.set_telegram_frontend_state(
                    principal,
                    scope,
                    &context.main.id,
                    Some(&side.id),
                    "side",
                )?;
            }
            ChatMode::Side => self.storage.set_telegram_frontend_state(
                principal,
                scope,
                &context.main.id,
                None,
                "main",
            )?,
        }
        self.context_for_telegram(principal, scope)
    }

    pub fn archive_and_recover_telegram(
        &self,
        principal: &str,
        scope: TelegramScope,
        id: &str,
    ) -> Result<SessionRecord> {
        if !self.session_in_scope(principal, id, scope)? {
            return Err(anyhow!("session is not in this Telegram topic"));
        }
        let target = self
            .storage
            .session(principal, id)?
            .ok_or_else(|| anyhow!("session not found"))?;
        if target.is_side {
            return Err(anyhow!("side sessions are not managed by /sessions"));
        }
        self.storage.archive_session(principal, id)?;
        let fallback = self
            .storage
            .list_main_sessions_in_telegram_scope(principal, scope, 1, 0, false)?
            .into_iter()
            .next()
            .unwrap_or(self.new_telegram_main(principal, scope)?);
        self.storage
            .set_telegram_frontend_state(principal, scope, &fallback.id, None, "main")?;
        Ok(fallback)
    }

    /// Typed real-delete operation used by Telegram, CLI and WebUI adapters.
    /// The storage transaction updates the relevant frontend pointer and
    /// returns the raw attachment paths for post-commit lifecycle cleanup.
    pub fn delete_and_recover(
        &self,
        principal: &str,
        id: &str,
    ) -> Result<(SessionRecord, SessionDeletionResult)> {
        let result = self
            .storage
            .delete_session_and_recover(principal, id, None)?;
        let active = self
            .storage
            .session(principal, &result.active_session_id)?
            .ok_or_else(|| anyhow!("replacement session disappeared after deletion"))?;
        Ok((active, result))
    }

    pub fn delete_and_recover_telegram(
        &self,
        principal: &str,
        scope: TelegramScope,
        id: &str,
    ) -> Result<(SessionRecord, SessionDeletionResult)> {
        if !self.session_in_scope(principal, id, scope)? {
            return Err(anyhow!("session is not in this Telegram topic"));
        }
        let result = self
            .storage
            .delete_session_and_recover(principal, id, Some(scope))?;
        let active = self
            .storage
            .session(principal, &result.active_session_id)?
            .ok_or_else(|| anyhow!("replacement Telegram session disappeared after deletion"))?;
        Ok((active, result))
    }

    pub fn list_telegram_page(
        &self,
        principal: &str,
        scope: TelegramScope,
        page: usize,
        page_size: usize,
    ) -> Result<(Vec<SessionRecord>, usize, usize)> {
        let page_size = page_size.max(1);
        let total = self
            .storage
            .count_main_sessions_in_telegram_scope(principal, scope)?;
        let pages = total.max(1).div_ceil(page_size).max(1);
        let current = page.clamp(1, pages);
        Ok((
            self.storage.list_main_sessions_in_telegram_scope(
                principal,
                scope,
                page_size,
                (current - 1) * page_size,
                false,
            )?,
            pages,
            current,
        ))
    }

    fn session_in_scope(
        &self,
        principal: &str,
        session_id: &str,
        scope: TelegramScope,
    ) -> Result<bool> {
        Ok(self
            .storage
            .telegram_scope_for_session(principal, session_id)?
            .is_some_and(|(chat, thread)| {
                chat == scope.chat_id && thread == scope.message_thread_id
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_never_writes_main() {
        let db = Arc::new(Storage::open_memory().unwrap());
        let sm = SessionManager::new(db.clone());
        let main = sm.ensure_default_session("u").unwrap();
        sm.switch_main("u", &main.id).unwrap();
        db.append_message("u", &main.id, "user", "main").unwrap();
        sm.toggle_side("u").unwrap();
        sm.append_user("u", "side only").unwrap();
        let side = sm.context_for("u").unwrap();
        sm.append_assistant_to("u", &side.active.id, "side answer")
            .unwrap();
        let main_msgs = db.messages("u", &main.id).unwrap();
        assert_eq!(main_msgs.len(), 1);
        assert!(!main_msgs.iter().any(|m| m.content.contains("side")));
        assert_eq!(sm.agent_context("u").unwrap().len(), 3);
    }

    #[test]
    fn no_nested_side() {
        let db = Arc::new(Storage::open_memory().unwrap());
        let sm = SessionManager::new(db);
        sm.ensure_default_session("x").unwrap();
        assert_eq!(sm.toggle_side("x").unwrap().mode, ChatMode::Side);
        assert_eq!(sm.toggle_side("x").unwrap().mode, ChatMode::Main);
    }

    #[test]
    fn archive_active_recovers_to_another_main_session() {
        let db = Arc::new(Storage::open_memory().unwrap());
        let sm = SessionManager::new(db.clone());
        let first = sm.ensure_default_session("x").unwrap();
        sm.switch_main("x", &first.id).unwrap();
        let second = sm.create_and_switch("x").unwrap();
        let recovered = sm.archive_and_recover("x", &second.id).unwrap();
        assert_ne!(recovered.id, second.id);
        assert!(db.session("x", &second.id).unwrap().unwrap().archived);
    }

    #[test]
    fn delete_active_main_removes_descendants_and_preserves_owner_global_state() {
        let db = Arc::new(Storage::open_memory().unwrap());
        let manager = SessionManager::new(db.clone());
        let owner = "owner:delete";
        let main = manager.ensure_default_session(owner).unwrap();
        manager.switch_main(owner, &main.id).unwrap();
        let side = manager.toggle_side(owner).unwrap().active;
        manager.toggle_side(owner).unwrap();

        crate::memory::MemoryStore::new(db.clone())
            .upsert(
                owner,
                crate::memory::MemoryScope::User,
                "preferences",
                "delete_regression",
                "preserve this",
                1.0,
                "test",
                None,
            )
            .unwrap();
        crate::skills::SkillStore::new(db.clone())
            .create_or_update(
                owner,
                crate::skills::SkillCandidate {
                    name: "preserve skill".into(),
                    summary: "owner-global skill".into(),
                    when_to_use: "during deletion tests".into(),
                    prerequisites: String::new(),
                    procedure: "verify the owner-global record survives".into(),
                    pitfalls: String::new(),
                    verification: "assert the record remains".into(),
                },
                Some(&main.id),
            )
            .unwrap();
        let profile = crate::providers::ProviderProfileStore::new(db.clone())
            .create(crate::storage::ProviderProfileInput {
                profile_id: None,
                owner_id: owner.into(),
                alias: "preserve-profile".into(),
                endpoint: "https://example.test/v1".into(),
                protocol: "openai_chat_completions".into(),
                credential_ref: None,
                safe_headers_json: "{}".into(),
                ..Default::default()
            })
            .unwrap();

        let (replacement, _deleted) = manager.delete_and_recover(owner, &main.id).unwrap();
        assert_ne!(replacement.id, main.id);
        assert!(!replacement.is_side);
        assert!(!replacement.yolo_mode);
        assert!(db.session(owner, &main.id).unwrap().is_none());
        assert!(db.session(owner, &side.id).unwrap().is_none());
        assert_eq!(
            crate::memory::MemoryStore::new(db.clone())
                .list(owner, Some(crate::memory::MemoryScope::User), 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::skills::SkillStore::new(db.clone())
                .list(owner, 10)
                .unwrap()
                .len(),
            1
        );
        assert!(crate::providers::ProviderProfileStore::new(db)
            .get(owner, &profile.profile_id)
            .unwrap()
            .is_some());
    }

    #[test]
    fn telegram_session_manager_paginates_thirteen_main_sessions_as_five_five_three() {
        let db = Arc::new(Storage::open_memory().unwrap());
        let manager = SessionManager::new(db);
        let owner = "owner:pages";
        let scope = TelegramScope::new(100, Some(10));
        manager.ensure_telegram_session(owner, scope).unwrap();
        for _ in 1..13 {
            manager.create_and_switch_telegram(owner, scope).unwrap();
        }

        let first = manager.list_telegram_page(owner, scope, 1, 5).unwrap();
        let second = manager.list_telegram_page(owner, scope, 2, 5).unwrap();
        let third = manager.list_telegram_page(owner, scope, 3, 5).unwrap();
        assert_eq!((first.0.len(), first.1, first.2), (5, 3, 1));
        assert_eq!((second.0.len(), second.1, second.2), (5, 3, 2));
        assert_eq!((third.0.len(), third.1, third.2), (3, 3, 3));
    }

    #[test]
    fn principal_cannot_switch_to_or_list_another_principals_sessions() {
        let db = Arc::new(Storage::open_memory().unwrap());
        let sm = SessionManager::new(db);
        let a = sm.ensure_default_session("a").unwrap();
        let b = sm.ensure_default_session("b").unwrap();
        assert_ne!(a.id, b.id);
        assert!(sm.switch_main("a", &b.id).is_err());
        let rows = sm.list_page("a", 1, 5).unwrap().0;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner_principal, "a");
    }

    #[test]
    fn side_session_inherits_parent_owner_and_cannot_cross_principals() {
        let db = Arc::new(Storage::open_memory().unwrap());
        let sm = SessionManager::new(db.clone());
        let main = sm.ensure_default_session("a").unwrap();
        sm.switch_main("a", &main.id).unwrap();
        let side = sm.toggle_side("a").unwrap().active;
        assert_eq!(side.owner_principal, "a");
        assert_eq!(side.parent_id.as_deref(), Some(main.id.as_str()));
        assert!(db.session("b", &side.id).unwrap().is_none());
        assert!(db
            .append_message("b", &side.id, "user", "steal side chat")
            .is_err());
        assert!(db.messages("b", &side.id).unwrap().is_empty());
    }

    #[test]
    fn telegram_topics_have_independent_sessions_lists_and_yolo_state() {
        let storage = Arc::new(Storage::open_memory().unwrap());
        let manager = SessionManager::new(storage.clone());
        let owner = "telegram:100:7";
        let topic_a = TelegramScope::new(100, Some(10));
        let topic_b = TelegramScope::new(100, Some(20));
        let first_a = manager.context_for_telegram(owner, topic_a).unwrap().main;
        let first_b = manager.context_for_telegram(owner, topic_b).unwrap().main;
        assert_ne!(first_a.id, first_b.id);
        let second_a = manager.create_and_switch_telegram(owner, topic_a).unwrap();
        storage.set_session_yolo(owner, &second_a.id, true).unwrap();
        assert!(
            manager
                .context_for_telegram(owner, topic_a)
                .unwrap()
                .active
                .yolo_mode
        );
        assert!(
            !manager
                .context_for_telegram(owner, topic_b)
                .unwrap()
                .active
                .yolo_mode
        );
        let rows_a = manager.list_telegram_page(owner, topic_a, 1, 5).unwrap().0;
        let rows_b = manager.list_telegram_page(owner, topic_b, 1, 5).unwrap().0;
        assert_eq!(rows_a.len(), 2);
        assert_eq!(rows_b.len(), 1);
        assert!(rows_a.iter().all(|row| row.id != first_b.id));
        assert!(rows_b.iter().all(|row| row.id != first_a.id));

        let side = manager.toggle_telegram_side(owner, topic_a).unwrap().active;
        assert!(side.is_side);
        assert!(!side.yolo_mode, "side chat must default YOLO OFF");
        assert_eq!(
            storage.telegram_scope_for_session(owner, &side.id).unwrap(),
            Some((100, Some(10)))
        );
    }

    #[test]
    fn new_session_inherits_from_active_main_within_same_scope() {
        let storage = Arc::new(Storage::open_memory().unwrap());
        let manager = SessionManager::new(storage.clone());

        let owner_a = "user:alice";
        let owner_b = "user:bob";
        let scope_1 = TelegramScope::new(100, Some(1));
        let scope_2 = TelegramScope::new(100, Some(2));

        // 1. First session gets defaults: custom / None / default
        let first_a1 = manager.ensure_telegram_session(owner_a, scope_1).unwrap();
        assert_eq!(first_a1.provider, "custom");
        assert_eq!(first_a1.account_id, None);
        assert_eq!(first_a1.model, "default");

        // First session for owner_a in generic scope also gets defaults
        let first_gen = manager.ensure_default_session(owner_a).unwrap();
        assert_eq!(first_gen.provider, "custom");
        assert_eq!(first_gen.account_id, None);
        assert_eq!(first_gen.model, "default");

        // Add a message to first_a1 to verify history is NOT inherited
        storage
            .append_message(owner_a, &first_a1.id, "user", "hello in first")
            .unwrap();
        assert_eq!(
            storage
                .session_messages(owner_a, &first_a1.id)
                .unwrap()
                .len(),
            1
        );

        // Update active session in scope_1 to custom provider profile and new model
        storage
            .set_session_provider(
                owner_a,
                &first_a1.id,
                "custom",
                Some("profile_123"),
                "gpt-5-turbo",
            )
            .unwrap();

        // 2. /new (create_and_switch_telegram) inherits provider/account/model from active main in same scope
        let second_a1 = manager
            .create_and_switch_telegram(owner_a, scope_1)
            .unwrap();
        assert_eq!(second_a1.provider, "custom");
        assert_eq!(second_a1.account_id.as_deref(), Some("profile_123"));
        assert_eq!(second_a1.model, "gpt-5-turbo");

        // Verify NO history inheritance into the new session
        let msgs = storage.session_messages(owner_a, &second_a1.id).unwrap();
        assert!(msgs.is_empty());

        // 3. No cross-topic inheritance: scope_2 on owner_a should get default
        let first_a2 = manager.ensure_telegram_session(owner_a, scope_2).unwrap();
        assert_eq!(first_a2.provider, "custom");
        assert_eq!(first_a2.account_id, None);
        assert_eq!(first_a2.model, "default");

        // 4. No cross-owner inheritance: owner_b on scope_1 should get default
        let first_b1 = manager.ensure_telegram_session(owner_b, scope_1).unwrap();
        assert_eq!(first_b1.provider, "custom");
        assert_eq!(first_b1.account_id, None);
        assert_eq!(first_b1.model, "default");

        // 5. Test generic (non-telegram) create_and_switch inheritance
        storage
            .set_session_provider(
                owner_a,
                &first_gen.id,
                "custom",
                Some("profile_gen"),
                "claude-sonnet",
            )
            .unwrap();
        let second_gen = manager.create_and_switch(owner_a).unwrap();
        assert_eq!(second_gen.provider, "custom");
        assert_eq!(second_gen.account_id.as_deref(), Some("profile_gen"));
        assert_eq!(second_gen.model, "claude-sonnet");
        assert!(storage
            .session_messages(owner_a, &second_gen.id)
            .unwrap()
            .is_empty());
    }
}
