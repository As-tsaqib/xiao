use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelegramCommandDefinition {
    pub name: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelegramCommandAlias {
    pub alias: &'static str,
    pub canonical: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
}

pub struct TelegramCommandRegistry;

impl TelegramCommandRegistry {
    pub const PUBLIC: [TelegramCommandDefinition; 15] = [
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
            description: "Configure the Custom AI endpoint",
        },
        TelegramCommandDefinition {
            name: "provider",
            description: "Show active AI provider profile summary",
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
            name: "retry",
            description: "Retry the latest request",
        },
        TelegramCommandDefinition {
            name: "yolo",
            description: "Toggle YOLO for this session",
        },
        TelegramCommandDefinition {
            name: "stop",
            description: "Stop the current task",
        },
        TelegramCommandDefinition {
            name: "skills",
            description: "View skills Xiao learned successfully",
        },
        TelegramCommandDefinition {
            name: "tools",
            description: "Show available capabilities",
        },
    ];

    pub const ALIASES: [TelegramCommandAlias; 4] = [
        TelegramCommandAlias {
            alias: "n",
            canonical: "new",
        },
        TelegramCommandAlias {
            alias: "s",
            canonical: "sessions",
        },
        TelegramCommandAlias {
            alias: "r",
            canonical: "retry",
        },
        TelegramCommandAlias {
            alias: "y",
            canonical: "yolo",
        },
    ];

    pub fn public() -> &'static [TelegramCommandDefinition] {
        &Self::PUBLIC
    }

    pub fn aliases() -> &'static [TelegramCommandAlias] {
        &Self::ALIASES
    }

    /// Canonicalize only the supported Telegram registry. Management and
    /// historical compatibility routes intentionally do not live here: the
    /// Telegram parser must be able to prove that they are unknown.
    pub fn canonical(name: &str) -> Option<&'static str> {
        Self::PUBLIC
            .iter()
            .find(|definition| definition.name == name)
            .map(|definition| definition.name)
            .or_else(|| {
                Self::ALIASES
                    .iter()
                    .find(|alias| alias.alias == name)
                    .map(|alias| alias.canonical)
            })
    }

    pub fn help_text() -> String {
        let primary = Self::PUBLIC
            .iter()
            .map(|definition| {
                let alias = Self::ALIASES
                    .iter()
                    .find(|alias| alias.canonical == definition.name)
                    .map(|alias| format!(" (/{})", alias.alias))
                    .unwrap_or_default();
                format!("/{}{} — {}", definition.name, alias, definition.description)
            })
            .collect::<Vec<_>>();
        primary.join("\n")
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
                "start", "help", "login", "provider", "model", "new", "sessions", "btw", "status",
                "context", "retry", "yolo", "stop", "skills", "tools",
            ]
        );
        for removed in [
            "cancel",
            "memory",
            "doctor",
            "approvals",
            "approve",
            "deny",
            "session",
            "account",
            "settings",
            "usage",
            "env",
            "about",
            "logout",
        ] {
            assert!(TelegramCommandRegistry::canonical(removed).is_none());
        }
        assert_eq!(TelegramCommandRegistry::canonical("n"), Some("new"));
        assert_eq!(TelegramCommandRegistry::canonical("s"), Some("sessions"));
        assert_eq!(TelegramCommandRegistry::canonical("r"), Some("retry"));
        assert_eq!(TelegramCommandRegistry::canonical("y"), Some("yolo"));
    }

    #[test]
    fn help_and_native_menu_share_the_same_order_and_entries() {
        let help = TelegramCommandRegistry::help_text();
        let native = TelegramCommandRegistry::bot_commands();
        assert_eq!(native.len(), TelegramCommandRegistry::PUBLIC.len());
        for command in &native {
            assert_eq!(help.matches(&format!("/{} ", command.command)).count(), 1);
        }
        assert_eq!(native.len(), 15);
        assert!(!native.iter().any(|command| command.command == "n"));
    }
}
