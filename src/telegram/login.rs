use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::presentation::{Action, Block, View};
use crate::providers::ProviderCapabilities;

use super::{paginator::Paginator, TelegramScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomLoginPhase {
    Endpoint,
    ApiKey,
    Alias,
    Models,
    Confirm,
}

/// In-memory, expiring wizard state. The API key is zeroized on replacement
/// or drop and is never embedded in a View/callback payload.
pub struct CustomLoginWizard {
    pub id: String,
    pub owner_user_id: i64,
    pub scope: TelegramScope,
    pub menu_id: String,
    pub expires_at: Instant,
    pub phase: CustomLoginPhase,
    pub endpoint: Option<String>,
    pub api_key: Option<Zeroizing<String>>,
    pub alias: String,
    pub models: Vec<String>,
    pub selected_index: Option<usize>,
    pub capability: Option<ProviderCapabilities>,
    pub page: usize,
}

impl CustomLoginWizard {
    pub fn valid_for(&self, owner_user_id: i64, scope: TelegramScope, menu_id: &str) -> bool {
        self.owner_user_id == owner_user_id
            && self.scope == scope
            && self.menu_id == menu_id
            && self.expires_at > Instant::now()
    }
}

pub struct CustomLoginStore {
    ttl: Duration,
    inner: Mutex<HashMap<String, Arc<AsyncMutex<CustomLoginWizard>>>>,
}

impl CustomLoginStore {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn begin(
        &self,
        scope: TelegramScope,
        owner_user_id: i64,
        menu_id: String,
    ) -> Arc<AsyncMutex<CustomLoginWizard>> {
        self.purge();
        let id = Uuid::new_v4().simple().to_string()[..12].to_owned();
        let wizard = Arc::new(AsyncMutex::new(CustomLoginWizard {
            id: id.clone(),
            owner_user_id,
            scope,
            menu_id,
            expires_at: Instant::now() + self.ttl,
            phase: CustomLoginPhase::Endpoint,
            endpoint: None,
            api_key: None,
            alias: "custom".into(),
            models: Vec::new(),
            selected_index: None,
            capability: None,
            page: 1,
        }));
        self.inner.lock().unwrap().insert(id, wizard.clone());
        wizard
    }

    pub fn get(&self, id: &str) -> Option<Arc<AsyncMutex<CustomLoginWizard>>> {
        self.purge();
        self.inner.lock().unwrap().get(id).cloned()
    }

    pub fn remove(&self, id: &str) {
        self.inner.lock().unwrap().remove(id);
    }

    fn purge(&self) {
        let now = Instant::now();
        self.inner.lock().unwrap().retain(|_, wizard| {
            wizard
                .try_lock()
                .map(|state| state.expires_at > now)
                .unwrap_or(true)
        });
    }
}

pub fn endpoint_view(id: &str) -> View {
    View {
        title: Some("CUSTOM LOGIN · ENDPOINT".into()),
        blocks: vec![Block::Paragraph {
            text: "Send the OpenAI-compatible endpoint URL (for example https://host.example/v1). Credentials in URLs are rejected.".into(),
        }],
        actions: vec![vec![Action::back(), Action::close()]],
        side_mode: false,
    }
    .with_internal_hint(id)
}

pub fn api_key_view(id: &str) -> View {
    View {
        title: Some("CUSTOM LOGIN · API KEY".into()),
        blocks: vec![Block::Paragraph {
            text: "Send the API key. Xiao will not echo it and will try to delete that Telegram message. If the endpoint needs no key, choose Skip.".into(),
        }],
        actions: vec![vec![
            Action::command("Skip", format!("/_custom:{id}:skip_key")),
            Action::close(),
        ]],
        side_mode: false,
    }
}

pub fn alias_view(id: &str) -> View {
    View {
        title: Some("CUSTOM LOGIN · ALIAS".into()),
        blocks: vec![Block::Paragraph {
            text: "Send an optional short provider alias, or use the default: custom.".into(),
        }],
        actions: vec![vec![
            Action::command("Use custom", format!("/_custom:{id}:default_alias")),
            Action::close(),
        ]],
        side_mode: false,
    }
}

pub fn model_view(wizard: &CustomLoginWizard) -> View {
    let paginator = Paginator::new(wizard.models.len(), wizard.page, 5);
    let indexes = paginator.range().collect::<Vec<_>>();
    let rows = indexes
        .iter()
        .enumerate()
        .map(|(number, index)| vec![(number + 1).to_string(), wizard.models[*index].clone()])
        .collect();
    let mut actions = Vec::new();
    if !indexes.is_empty() {
        actions.push(
            indexes
                .iter()
                .enumerate()
                .map(|(number, index)| {
                    Action::command(
                        (number + 1).to_string(),
                        format!("/_custom:{}:select:{index}", wizard.id),
                    )
                })
                .collect(),
        );
    }
    actions.push(vec![
        Action::command(
            "‹",
            format!("/_custom:{}:page:{}", wizard.id, paginator.previous()),
        ),
        Action::noop(format!("Page {}/{}", paginator.page(), paginator.pages())),
        Action::command(
            "›",
            format!("/_custom:{}:page:{}", wizard.id, paginator.next()),
        ),
    ]);
    actions.push(vec![Action::close()]);
    View {
        title: Some("CUSTOM LOGIN · MODELS".into()),
        blocks: vec![Block::Table {
            headers: vec!["No".into(), "Model".into()],
            rows,
        }],
        actions,
        side_mode: false,
    }
}

pub fn confirmation_view(wizard: &CustomLoginWizard) -> View {
    let selected = wizard
        .selected_index
        .and_then(|index| wizard.models.get(index))
        .cloned()
        .unwrap_or_else(|| "—".into());
    View {
        title: Some("CUSTOM LOGIN · CONFIRM".into()),
        blocks: vec![Block::Table {
            headers: vec!["Field".into(), "Value".into()],
            rows: vec![
                vec![
                    "Endpoint".into(),
                    wizard.endpoint.clone().unwrap_or_else(|| "—".into()),
                ],
                vec![
                    "API key".into(),
                    if wizard.api_key.is_some() {
                        "configured".into()
                    } else {
                        "none".into()
                    },
                ],
                vec!["Alias".into(), wizard.alias.clone()],
                vec!["Model".into(), selected],
                vec![
                    "Agent capability".into(),
                    wizard
                        .capability
                        .as_ref()
                        .map(|capability| capability.tool_protocol.as_str().to_owned())
                        .unwrap_or_else(|| "not probed".into()),
                ],
            ],
        }],
        actions: vec![
            vec![Action::command(
                "Confirm",
                format!("/_custom:{}:confirm", wizard.id),
            )],
            vec![Action::back(), Action::close()],
        ],
        side_mode: false,
    }
}

// Retains endpoint-view signature symmetry without placing the wizard id in
// visible text. Internal callback commands contain only the opaque state id.
trait InternalHint {
    fn with_internal_hint(self, _id: &str) -> Self;
}
impl InternalHint for View {
    fn with_internal_hint(self, _id: &str) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_picker_has_at_most_five_rows_and_no_model_blobs_in_callbacks() {
        let store = CustomLoginStore::new(Duration::from_secs(60));
        let wizard = store.begin(TelegramScope::new(1, Some(9)), 2, "menu".into());
        let mut wizard = wizard.blocking_lock();
        wizard.models = (0..12).map(|index| format!("model-{index}")).collect();
        wizard.page = 2;
        let view = model_view(&wizard);
        let Block::Table { rows, .. } = &view.blocks[0] else {
            panic!("model table expected")
        };
        assert_eq!(rows.len(), 5);
        for action in view.actions.iter().flatten() {
            if let Some(command) = action.callback_command() {
                assert!(!command.contains("model-"));
            }
        }
    }

    #[test]
    fn wizard_state_requires_owner_chat_topic_menu_and_unexpired_state() {
        let scope = TelegramScope::new(100, Some(10));
        let store = CustomLoginStore::new(Duration::from_secs(60));
        let wizard = store.begin(scope, 7, "menu-a".into());
        let mut wizard = wizard.blocking_lock();
        assert!(wizard.valid_for(7, scope, "menu-a"));
        assert!(!wizard.valid_for(8, scope, "menu-a"));
        assert!(!wizard.valid_for(7, TelegramScope::new(101, Some(10)), "menu-a"));
        assert!(!wizard.valid_for(7, TelegramScope::new(100, Some(20)), "menu-a"));
        assert!(!wizard.valid_for(7, scope, "menu-b"));
        wizard.expires_at = Instant::now();
        assert!(!wizard.valid_for(7, scope, "menu-a"));
    }

    #[test]
    fn expired_wizard_is_purged_from_lookup() {
        let store = CustomLoginStore::new(Duration::ZERO);
        let wizard = store.begin(TelegramScope::new(1, None), 2, "menu".into());
        let id = wizard.blocking_lock().id.clone();
        drop(wizard);
        assert!(store.get(&id).is_none());
    }

    #[test]
    fn api_key_never_enters_visible_views_or_callback_commands() {
        let store = CustomLoginStore::new(Duration::from_secs(60));
        let wizard = store.begin(TelegramScope::new(1, Some(2)), 3, "menu".into());
        let mut wizard = wizard.blocking_lock();
        wizard.endpoint = Some("https://example.test/v1".into());
        wizard.api_key = Some(Zeroizing::new("super-secret-api-key".into()));
        wizard.models = vec!["model-a".into()];
        wizard.selected_index = Some(0);
        let serialized = serde_json::to_string(&confirmation_view(&wizard)).unwrap();
        assert!(!serialized.contains("super-secret-api-key"));
        assert!(serialized.contains("configured"));
        for action in model_view(&wizard).actions.iter().flatten() {
            assert!(!action
                .callback_command()
                .unwrap_or_default()
                .contains("super-secret-api-key"));
        }
    }
}
