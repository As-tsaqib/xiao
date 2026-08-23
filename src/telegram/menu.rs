use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::{
    presentation::{Action, ActionTarget, View},
    telegram::TelegramScope,
};

#[derive(Debug, Clone)]
pub struct MenuSession {
    pub id: String,
    pub chat_id: i64,
    pub message_thread_id: Option<i64>,
    pub message_id: i64,
    pub owner_user_id: i64,
    pub current_view: View,
    pub history: Vec<View>,
    pub revision: u64,
    pub expires_at: Instant,
    pub pending_input: Option<String>,
}

pub struct MenuStore {
    ttl: Duration,
    inner: Mutex<HashMap<String, Arc<AsyncMutex<MenuSession>>>>,
}

impl MenuStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn prepare(
        &self,
        chat_id: i64,
        owner_user_id: i64,
        view: View,
    ) -> Arc<AsyncMutex<MenuSession>> {
        self.prepare_scoped(TelegramScope::new(chat_id, None), owner_user_id, view)
    }

    pub fn prepare_scoped(
        &self,
        scope: TelegramScope,
        owner_user_id: i64,
        view: View,
    ) -> Arc<AsyncMutex<MenuSession>> {
        self.purge();
        let id = Uuid::new_v4().simple().to_string()[..10].to_owned();
        Arc::new(AsyncMutex::new(MenuSession {
            id,
            chat_id: scope.chat_id,
            message_thread_id: scope.message_thread_id,
            message_id: 0,
            owner_user_id,
            current_view: view,
            history: vec![],
            revision: 1,
            expires_at: Instant::now() + self.ttl,
            pending_input: None,
        }))
    }

    pub fn insert(&self, menu: Arc<AsyncMutex<MenuSession>>, id: String) {
        self.inner.lock().unwrap().insert(id, menu);
    }
    pub fn get(&self, id: &str) -> Option<Arc<AsyncMutex<MenuSession>>> {
        self.purge();
        self.inner.lock().unwrap().get(id).cloned()
    }
    pub fn remove(&self, id: &str) {
        self.inner.lock().unwrap().remove(id);
    }

    pub fn pending_for(
        &self,
        chat_id: i64,
        owner_user_id: i64,
    ) -> Option<Arc<AsyncMutex<MenuSession>>> {
        self.pending_for_scope(TelegramScope::new(chat_id, None), owner_user_id)
    }

    pub fn pending_for_scope(
        &self,
        scope: TelegramScope,
        owner_user_id: i64,
    ) -> Option<Arc<AsyncMutex<MenuSession>>> {
        self.purge();
        self.inner.lock().unwrap().values().find_map(|menu| {
            let guard = menu.try_lock().ok()?;
            if guard.chat_id == scope.chat_id
                && guard.message_thread_id == scope.message_thread_id
                && guard.owner_user_id == owner_user_id
                && guard.pending_input.is_some()
            {
                Some(menu.clone())
            } else {
                None
            }
        })
    }

    fn purge(&self) {
        let now = Instant::now();
        self.inner
            .lock()
            .unwrap()
            .retain(|_, menu| menu.try_lock().map(|g| g.expires_at > now).unwrap_or(true));
    }
}

pub fn callback_data(menu_id: &str, revision: u64, index: usize) -> String {
    format!("m:{menu_id}:{revision:x}:{index:x}")
}

pub fn parse_callback(data: &str) -> Result<(String, u64, usize)> {
    let p = data.split(':').collect::<Vec<_>>();
    if p.len() != 4 || p[0] != "m" {
        return Err(anyhow!("invalid callback"));
    }
    Ok((
        p[1].into(),
        u64::from_str_radix(p[2], 16)?,
        usize::from_str_radix(p[3], 16)?,
    ))
}

pub fn keyboard(view: &View, menu_id: &str, revision: u64) -> Value {
    let mut callback_index = 0usize;
    let rows = view
        .actions
        .iter()
        .map(|row| {
            row.iter().map(|action| match &action.target {
            ActionTarget::Url(url) => json!({"text":action.label,"url":url}),
            _ => {
                let index = callback_index;
                callback_index += 1;
                json!({"text":action.label,"callback_data":callback_data(menu_id, revision, index)})
            }
        }).collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    json!({"inline_keyboard":rows})
}

pub fn action_at(view: &View, index: usize) -> Option<Action> {
    view.actions
        .iter()
        .flatten()
        .filter(|a| !matches!(a.target, ActionTarget::Url(_)))
        .nth(index)
        .cloned()
}

#[async_trait::async_trait]
pub trait EditTransport: Send + Sync {
    async fn edit_rich_surface(
        &self,
        chat_id: i64,
        message_id: i64,
        rich: Value,
        markup: Value,
    ) -> Result<()>;
    async fn edit_plain_surface(
        &self,
        chat_id: i64,
        message_id: i64,
        text: String,
        markup: Value,
    ) -> Result<()>;
    async fn send_rich_replacement(
        &self,
        scope: TelegramScope,
        rich: Value,
        markup: Value,
    ) -> Result<i64>;
    async fn send_plain_replacement(
        &self,
        scope: TelegramScope,
        text: String,
        markup: Value,
    ) -> Result<i64>;
    async fn retire_keyboard(&self, chat_id: i64, message_id: i64) -> Result<()>;
}

/// Edit-first/fallback: rich edit -> plain edit -> rich replacement -> plain replacement.
/// A replacement always retires the stale keyboard on the previous message when possible.
pub async fn edit_first<T: EditTransport>(
    transport: &T,
    menu: &mut MenuSession,
    rich: Value,
    plain: String,
    markup: Value,
) -> Result<()> {
    match transport
        .edit_rich_surface(menu.chat_id, menu.message_id, rich.clone(), markup.clone())
        .await
    {
        Ok(()) => return Ok(()),
        Err(error)
            if error
                .to_string()
                .to_ascii_lowercase()
                .contains("message is not modified") =>
        {
            return Ok(())
        }
        Err(_) => {}
    }
    if transport
        .edit_plain_surface(menu.chat_id, menu.message_id, plain.clone(), markup.clone())
        .await
        .is_ok()
    {
        return Ok(());
    }

    let old = menu.message_id;
    let new_id = match transport
        .send_rich_replacement(
            TelegramScope::new(menu.chat_id, menu.message_thread_id),
            rich,
            markup.clone(),
        )
        .await
    {
        Ok(id) => id,
        Err(_) => {
            transport
                .send_plain_replacement(
                    TelegramScope::new(menu.chat_id, menu.message_thread_id),
                    plain,
                    markup,
                )
                .await?
        }
    };
    menu.message_id = new_id;
    let _ = transport.retire_keyboard(menu.chat_id, old).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn callbacks_stay_short_and_urls_do_not_consume_indexes() {
        let view = View {
            title: None,
            blocks: vec![],
            actions: vec![vec![
                Action::url("web", "https://example.com"),
                Action::command("go", "/status"),
            ]],
            side_mode: false,
        };
        let data = callback_data("1234567890", u64::MAX, 999);
        assert!(data.len() <= 64);
        assert_eq!(parse_callback(&data).unwrap().1, u64::MAX);
        assert_eq!(
            action_at(&view, 0).unwrap().callback_command(),
            Some("/status")
        );
    }

    #[tokio::test]
    async fn revision_can_reject_stale() {
        let store = MenuStore::new(Duration::from_secs(60));
        let menu = store.prepare(1, 2, View::info("x", "y"));
        let id = menu.lock().await.id.clone();
        store.insert(menu.clone(), id.clone());
        menu.lock().await.revision = 2;
        assert_ne!(store.get(&id).unwrap().lock().await.revision, 1);
    }

    #[tokio::test]
    async fn expired_menu_state_is_not_resolved() {
        let store = MenuStore::new(Duration::ZERO);
        let menu = store.prepare_scoped(TelegramScope::new(100, Some(10)), 7, View::info("x", "y"));
        let id = menu.lock().await.id.clone();
        store.insert(menu.clone(), id.clone());
        drop(menu);
        assert!(store.get(&id).is_none());
    }

    struct Fake {
        edits: AtomicUsize,
        sends: AtomicUsize,
        retired: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl EditTransport for Fake {
        async fn edit_rich_surface(&self, _: i64, _: i64, _: Value, _: Value) -> Result<()> {
            self.edits.fetch_add(1, Ordering::SeqCst);
            Err(anyhow!("can't edit"))
        }
        async fn edit_plain_surface(&self, _: i64, _: i64, _: String, _: Value) -> Result<()> {
            self.edits.fetch_add(1, Ordering::SeqCst);
            Err(anyhow!("can't edit plain"))
        }
        async fn send_rich_replacement(&self, _: TelegramScope, _: Value, _: Value) -> Result<i64> {
            self.sends.fetch_add(1, Ordering::SeqCst);
            Ok(99)
        }
        async fn send_plain_replacement(
            &self,
            _: TelegramScope,
            _: String,
            _: Value,
        ) -> Result<i64> {
            self.sends.fetch_add(1, Ordering::SeqCst);
            Ok(100)
        }
        async fn retire_keyboard(&self, _: i64, _: i64) -> Result<()> {
            self.retired.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn edit_failure_falls_back_to_replacement_and_retires_old() {
        let fake = Fake {
            edits: AtomicUsize::new(0),
            sends: AtomicUsize::new(0),
            retired: AtomicUsize::new(0),
        };
        let mut menu = MenuSession {
            id: "m".into(),
            chat_id: 1,
            message_thread_id: None,
            message_id: 5,
            owner_user_id: 7,
            current_view: View::default(),
            history: vec![],
            revision: 1,
            expires_at: Instant::now() + Duration::from_secs(1),
            pending_input: None,
        };
        edit_first(&fake, &mut menu, json!({}), "x".into(), json!({}))
            .await
            .unwrap();
        assert_eq!(menu.message_id, 99);
        assert_eq!(fake.edits.load(Ordering::SeqCst), 2);
        assert_eq!(fake.sends.load(Ordering::SeqCst), 1);
        assert_eq!(fake.retired.load(Ordering::SeqCst), 1);
    }
}
