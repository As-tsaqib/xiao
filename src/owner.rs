use serde::{Deserialize, Serialize};

/// Stable, installation-local identity for Xiao's single owner.
///
/// Telegram chat and topic identifiers deliberately never enter this value;
/// they belong to `TelegramScope` and only namespace conversations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct OwnerIdentity {
    pub owner_id: String,
    pub telegram_user_id: Option<i64>,
}

impl OwnerIdentity {
    pub fn telegram(user_id: i64) -> Self {
        Self {
            owner_id: format!("owner:telegram:{user_id}"),
            telegram_user_id: Some(user_id),
        }
    }

    pub fn local() -> Self {
        Self {
            owner_id: "owner:local".into(),
            telegram_user_id: None,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.owner_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_owner_is_independent_of_chat_and_topic() {
        assert_eq!(OwnerIdentity::telegram(42), OwnerIdentity::telegram(42));
        assert_eq!(OwnerIdentity::telegram(42).as_str(), "owner:telegram:42");
    }
}
