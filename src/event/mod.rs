use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppEvent {
    SessionChanged {
        principal: String,
        session_id: String,
    },
    SessionArchived {
        principal: String,
        session_id: String,
    },
    ProviderChanged {
        principal: String,
        provider: String,
    },
    AccountChanged {
        principal: String,
        account_id: Option<String>,
    },
    ModelChanged {
        principal: String,
        model: String,
    },
    AuthStarted {
        provider: String,
        transaction_id: String,
    },
    AuthCompleted {
        provider: String,
        transaction_id: String,
        account_id: String,
    },
    AuthFailed {
        provider: String,
        transaction_id: String,
        message: String,
    },
    ConfigReloaded,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<AppEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(16));
        Self { tx }
    }

    pub fn publish(&self, event: AppEvent) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.tx.subscribe()
    }
}
