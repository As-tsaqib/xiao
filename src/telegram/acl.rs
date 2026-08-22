use crate::config::TelegramAccess;

#[derive(Debug, Clone)]
pub struct AccessPolicy {
    pub allowed_chat_ids: Vec<i64>,
    pub allowed_user_ids: Vec<i64>,
}
impl From<&TelegramAccess> for AccessPolicy {
    fn from(v: &TelegramAccess) -> Self {
        Self {
            allowed_chat_ids: v.allowed_chat_ids.clone(),
            allowed_user_ids: v.allowed_user_ids.clone(),
        }
    }
}
impl AccessPolicy {
    /// ACL is evaluated before slash parsing, normal prompt handling, agent invocation, or tool work.
    pub fn allows(&self, chat_id: i64, user_id: Option<i64>, chat_type: &str) -> bool {
        if !self.allowed_chat_ids.contains(&chat_id) {
            return false;
        }
        if self.allowed_user_ids.is_empty() {
            return true;
        }
        match user_id {
            Some(id) => self.allowed_user_ids.contains(&id),
            None => chat_type == "private" && self.allowed_user_ids.is_empty(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocks_before_any_core_work() {
        let p = AccessPolicy {
            allowed_chat_ids: vec![1, -100],
            allowed_user_ids: vec![7],
        };
        assert!(p.allows(1, Some(7), "private"));
        assert!(!p.allows(1, Some(8), "private"));
        assert!(!p.allows(2, Some(7), "private"));
    }
}
