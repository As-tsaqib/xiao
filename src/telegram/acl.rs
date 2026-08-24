use crate::config::TelegramAccess;

#[derive(Debug, Clone)]
pub struct AccessPolicy {
    pub allowed_chat_ids: Vec<i64>,
    pub owner_user_id: Option<i64>,
    pub owner_resolution_required: bool,
}
impl From<&TelegramAccess> for AccessPolicy {
    fn from(v: &TelegramAccess) -> Self {
        let legacy_owner = if v.owner_user_id.is_none() && v.allowed_user_ids.len() == 1 {
            v.allowed_user_ids.first().copied()
        } else {
            None
        };
        Self {
            allowed_chat_ids: v.allowed_chat_ids.clone(),
            owner_user_id: v.owner_user_id.or(legacy_owner),
            owner_resolution_required: v.owner_user_id.is_none() && v.allowed_user_ids.len() > 1,
        }
    }
}
impl AccessPolicy {
    pub fn allows(&self, chat_id: i64, user_id: Option<i64>, _chat_type: &str) -> bool {
        // Fail closed while setup is incomplete or a multi-owner legacy config
        // awaits explicit resolution. allowed_chat_ids never grants identity.
        if self.owner_resolution_required {
            return false;
        }
        let Some(owner_user_id) = self.owner_user_id else {
            return false;
        };
        if user_id != Some(owner_user_id) {
            return false;
        }
        self.allowed_chat_ids.is_empty() || self.allowed_chat_ids.contains(&chat_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_is_required_and_chat_ids_only_restrict_location() {
        let p = AccessPolicy {
            allowed_chat_ids: vec![-100],
            owner_user_id: Some(7),
            owner_resolution_required: false,
        };
        assert!(p.allows(-100, Some(7), "supergroup"));
        assert!(!p.allows(-100, Some(8), "supergroup"));
        assert!(!p.allows(-200, Some(7), "supergroup"));
        assert!(!p.allows(-100, None, "supergroup"));
    }

    #[test]
    fn setup_and_ambiguous_legacy_states_fail_closed() {
        let missing = AccessPolicy {
            allowed_chat_ids: vec![],
            owner_user_id: None,
            owner_resolution_required: false,
        };
        assert!(!missing.allows(1, Some(7), "private"));
        let ambiguous = AccessPolicy {
            owner_resolution_required: true,
            ..missing
        };
        assert!(!ambiguous.allows(1, Some(7), "private"));
    }
}
