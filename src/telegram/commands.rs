use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelegramCommandDefinition {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
}

pub struct TelegramCommandRegistry;

impl TelegramCommandRegistry {
    pub const PUBLIC: [TelegramCommandDefinition; 19] = [
        TelegramCommandDefinition {
            name: "start",
            description: "Start Xiao",
        },
        TelegramCommandDefinition {
            name: "help",
            description: "Show Xiao commands",
        },
        TelegramCommandDefinition {
            name: "login",
            description: "Connect or configure an AI provider",
        },
        TelegramCommandDefinition {
            name: "logout",
            description: "Disconnect an AI account/provider",
        },
        TelegramCommandDefinition {
            name: "model",
            description: "View or change the active model",
        },
        TelegramCommandDefinition {
            name: "new",
            description: "Create a new session",
        },
        TelegramCommandDefinition {
            name: "sessions",
            description: "Manage sessions in this chat/topic",
        },
        TelegramCommandDefinition {
            name: "btw",
            description: "Enter or leave an isolated side chat",
        },
        TelegramCommandDefinition {
            name: "status",
            description: "Show current Xiao status",
        },
        TelegramCommandDefinition {
            name: "context",
            description: "Show current context usage/composition",
        },
        TelegramCommandDefinition {
            name: "cancel",
            description: "Cancel the current task",
        },
        TelegramCommandDefinition {
            name: "retry",
            description: "Retry the latest request",
        },
        TelegramCommandDefinition {
            name: "yolo",
            description: "Manage approval-free mode for this session",
        },
        TelegramCommandDefinition {
            name: "memory",
            description: "View/manage Xiao memory",
        },
        TelegramCommandDefinition {
            name: "skills",
            description: "View/manage Xiao skills",
        },
        TelegramCommandDefinition {
            name: "tools",
            description: "Show available/installable capabilities",
        },
        TelegramCommandDefinition {
            name: "doctor",
            description: "Diagnose Xiao runtime",
        },
        TelegramCommandDefinition {
            name: "about",
            description: "Show Xiao identity and environment",
        },
        TelegramCommandDefinition {
            name: "approvals",
            description: "Review pending approvals",
        },
    ];

    pub fn public() -> &'static [TelegramCommandDefinition] {
        &Self::PUBLIC
    }

    /// Canonicalize only supported public commands and intentional hidden
    /// compatibility/internal commands. `/provider`, `/settings`, `/usage`,
    /// and `/env` deliberately have no route.
    pub fn canonical(name: &str) -> Option<&'static str> {
        match name {
            "session" => Some("sessions"),
            "stop" => Some("cancel"),
            "account" => Some("account"),
            "approve" => Some("approve"),
            "deny" => Some("deny"),
            candidate
                if Self::PUBLIC
                    .iter()
                    .any(|definition| definition.name == candidate) =>
            {
                Self::PUBLIC
                    .iter()
                    .find(|definition| definition.name == candidate)
                    .map(|definition| definition.name)
            }
            _ => None,
        }
    }

    pub fn help_text() -> String {
        Self::PUBLIC
            .iter()
            .map(|definition| format!("/{} — {}", definition.name, definition.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn bot_commands() -> Vec<BotCommand> {
        Self::PUBLIC
            .iter()
            .map(|definition| BotCommand {
                command: definition.name.into(),
                description: definition.description.into(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_registry_is_exact_and_hidden_commands_are_not_advertised() {
        let names = TelegramCommandRegistry::public()
            .iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "start",
                "help",
                "login",
                "logout",
                "model",
                "new",
                "sessions",
                "btw",
                "status",
                "context",
                "cancel",
                "retry",
                "yolo",
                "memory",
                "skills",
                "tools",
                "doctor",
                "about",
                "approvals"
            ]
        );
        for removed in ["provider", "settings", "usage", "env"] {
            assert!(TelegramCommandRegistry::canonical(removed).is_none());
        }
        assert_eq!(
            TelegramCommandRegistry::canonical("session"),
            Some("sessions")
        );
        assert_eq!(TelegramCommandRegistry::canonical("stop"), Some("cancel"));
    }

    #[test]
    fn help_and_native_menu_share_the_same_order_and_entries() {
        let help = TelegramCommandRegistry::help_text();
        let native = TelegramCommandRegistry::bot_commands();
        assert_eq!(native.len(), TelegramCommandRegistry::PUBLIC.len());
        for command in native {
            assert_eq!(help.matches(&format!("/{} —", command.command)).count(), 1);
        }
    }
}
