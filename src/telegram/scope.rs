use serde::{Deserialize, Serialize};

/// A Telegram conversation namespace. Owner/user identity is authorization;
/// chat and optional forum topic determine where Xiao conversation state lives.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TelegramScope {
    pub chat_id: i64,
    pub message_thread_id: Option<i64>,
}

impl TelegramScope {
    pub const DEFAULT_THREAD_KEY: i64 = 0;

    pub const fn new(chat_id: i64, message_thread_id: Option<i64>) -> Self {
        Self {
            chat_id,
            message_thread_id,
        }
    }

    pub const fn thread_key(self) -> i64 {
        match self.message_thread_id {
            Some(value) => value,
            None => Self::DEFAULT_THREAD_KEY,
        }
    }

    pub fn label(self) -> String {
        self.message_thread_id
            .map(|thread| format!("chat {} · topic {}", self.chat_id, thread))
            .unwrap_or_else(|| format!("chat {} · default", self.chat_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_thread_uses_stable_default_key() {
        assert_eq!(TelegramScope::new(7, None).thread_key(), 0);
        assert_eq!(TelegramScope::new(7, Some(12)).thread_key(), 12);
    }
}
