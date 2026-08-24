use serde::{Deserialize, Serialize};

/// Stable, installation-local identity for Xiao's single owner.
///
/// Telegram authentication is deliberately not part of this value. The
/// current Telegram user is an `OwnerBinding` maintained by SQLite, while
/// chat/topic identifiers remain `TelegramScope` conversation namespaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct OwnerIdentity {
    pub owner_id: String,
}

impl OwnerIdentity {
    pub fn from_owner_id(owner_id: impl Into<String>) -> Self {
        Self {
            owner_id: owner_id.into(),
        }
    }

    /// Construct the only safe kind of new owner identity. Existing installs
    /// get their ID from the durable `installation_owner` row instead.
    pub fn new_installation() -> Self {
        Self::from_owner_id(format!("owner:installation:{}", uuid::Uuid::new_v4()))
    }

    pub fn as_str(&self) -> &str {
        &self.owner_id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerBinding {
    pub owner_id: String,
    pub kind: OwnerBindingKind,
    pub external_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnerBindingKind {
    TelegramUser,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_owner_has_no_telegram_identity_semantics() {
        let owner = OwnerIdentity::new_installation();
        assert!(owner.as_str().starts_with("owner:installation:"));
        assert!(!owner.as_str().starts_with("owner:telegram:"));
    }
}
