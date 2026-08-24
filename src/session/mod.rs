use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::Local;

use crate::storage::{MessageRecord, SessionRecord, Storage};
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
        self.storage
            .create_session(principal, &name, "custom", None, "default", false, None)
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
        let session = self
            .storage
            .create_session(principal, &name, "custom", None, "default", false, None)?;
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
}
