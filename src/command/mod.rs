use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};

use crate::{
    agent::{AgentAnswer, AgentEngine},
    app::{GatewayStatus, HealthState},
    auth::{AuthChallenge, AuthManager},
    config::AppConfig,
    event::{AppEvent, EventBus},
    memory::{MemoryScope, MemoryStore},
    presentation::{Action, Block, View},
    providers::{AgentEvent, ProviderRegistry, ProviderState},
    runtime::{CapabilityStatus, RuntimeState},
    session::{ChatMode, SessionManager},
    skills::SkillStore,
    storage::{AccountRecord, Storage},
    telegram::{commands::TelegramCommandRegistry, TelegramScope},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Start,
    Help {
        topic: Option<String>,
    },
    Login {
        provider: Option<String>,
    },
    CancelAuth {
        transaction: String,
    },
    Logout {
        account: Option<String>,
    },
    Account,
    UseAccount {
        account: String,
    },
    Model,
    ModelPicker {
        page: usize,
    },
    SetModel {
        model: String,
    },
    NewSession,
    Session {
        page: usize,
    },
    SessionDetail {
        session: String,
    },
    SwitchSession {
        session: String,
    },
    RequestRenameSession {
        session: String,
    },
    RenameSession {
        session: String,
        name: String,
    },
    ArchiveSession {
        session: String,
    },
    ToggleSideChat,
    Status,
    Context,
    Stop,
    Retry,
    Yolo {
        enabled: Option<bool>,
    },
    Memory {
        query: Option<String>,
    },
    RequestMemoryEdit {
        scope: String,
        category: String,
        key: String,
    },
    EditMemory {
        scope: String,
        category: String,
        key: String,
        value: String,
    },
    ForgetMemory {
        scope: String,
        category: String,
        key: String,
    },
    Skills {
        query: Option<String>,
    },
    SkillDetail {
        skill: String,
    },
    SetSkillEnabled {
        skill: String,
        enabled: bool,
    },
    ConfirmDeleteSkill {
        skill: String,
    },
    DeleteSkill {
        skill: String,
    },
    Tools,
    About,
    Approvals,
    Approve {
        request: String,
    },
    DenyApproval {
        request: String,
    },
    Doctor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum CommandResult {
    InfoView(View),
    ManagerView(View),
    Confirmation(View),
    InputRequest { view: View, command_prefix: String },
    StartedAuth(AuthChallenge),
    StartCustomLogin,
    Agent(AgentAnswer),
    NoContent,
}

pub fn parse(input: &str) -> Result<Option<Command>> {
    let text = input.trim();
    if text.is_empty() || !text.starts_with('/') {
        return Ok(None);
    }
    let mut parts = text[1..].split_whitespace();
    let name = parts
        .next()
        .unwrap_or_default()
        .split('@')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let args = parts.collect::<Vec<_>>();
    let one = || args.first().map(|s| (*s).to_owned());

    let canonical = TelegramCommandRegistry::canonical(&name)
        .ok_or_else(|| anyhow!("unknown command /{name}"))?;
    Ok(Some(match canonical {
        "start" => Command::Start,
        "new" => Command::NewSession,
        "btw" => Command::ToggleSideChat,
        "sessions" if args.first() == Some(&"detail") => Command::SessionDetail {
            session: args
                .get(1)
                .ok_or_else(|| anyhow!("session id required"))?
                .to_string(),
        },
        "sessions" if args.first() == Some(&"switch") => Command::SwitchSession {
            session: args
                .get(1)
                .ok_or_else(|| anyhow!("session id required"))?
                .to_string(),
        },
        "sessions" if args.first() == Some(&"archive") => Command::ArchiveSession {
            session: args
                .get(1)
                .ok_or_else(|| anyhow!("session id required"))?
                .to_string(),
        },
        "sessions" if args.first() == Some(&"rename") && args.len() == 2 => {
            Command::RequestRenameSession {
                session: args[1].to_string(),
            }
        }
        "sessions" if args.first() == Some(&"rename") => Command::RenameSession {
            session: args
                .get(1)
                .ok_or_else(|| anyhow!("session id required"))?
                .to_string(),
            name: args.iter().skip(2).copied().collect::<Vec<_>>().join(" "),
        },
        "sessions" => Command::Session {
            page: one().and_then(|s| s.parse().ok()).unwrap_or(1),
        },
        "model" if args.first() == Some(&"change") => Command::ModelPicker {
            page: args
                .get(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(1),
        },
        "model" if !args.is_empty() => Command::SetModel {
            model: args.join(" "),
        },
        "model" => Command::Model,
        "account" if args.first() == Some(&"use") => Command::UseAccount {
            account: args
                .get(1)
                .ok_or_else(|| anyhow!("account id required"))?
                .to_string(),
        },
        "account" if !args.is_empty() => Command::UseAccount {
            account: args[0].to_owned(),
        },
        "account" => Command::Account,
        "login" if args.first() == Some(&"cancel") => Command::CancelAuth {
            transaction: args
                .get(1)
                .ok_or_else(|| anyhow!("auth transaction id required"))?
                .to_string(),
        },
        "login" => Command::Login { provider: one() },
        "logout" => Command::Logout { account: one() },
        "status" => Command::Status,
        "context" => Command::Context,
        "cancel" => Command::Stop,
        "retry" => Command::Retry,
        "yolo" => Command::Yolo {
            enabled: match args.first().map(|value| value.to_ascii_lowercase()) {
                Some(value) if matches!(value.as_str(), "on" | "enable") => Some(true),
                Some(value) if matches!(value.as_str(), "off" | "disable") => Some(false),
                Some(_) => return Err(anyhow!("yolo expects enable or disable")),
                None => None,
            },
        },
        "memory" if args.first() == Some(&"forget") => Command::ForgetMemory {
            scope: args
                .get(1)
                .ok_or_else(|| anyhow!("memory scope required"))?
                .to_string(),
            category: args
                .get(2)
                .ok_or_else(|| anyhow!("memory category required"))?
                .to_string(),
            key: args
                .get(3)
                .ok_or_else(|| anyhow!("memory key required"))?
                .to_string(),
        },
        "memory" if args.first() == Some(&"edit") && args.len() == 4 => {
            Command::RequestMemoryEdit {
                scope: args[1].to_string(),
                category: args[2].to_string(),
                key: args[3].to_string(),
            }
        }
        "memory" if args.first() == Some(&"edit") => Command::EditMemory {
            scope: args
                .get(1)
                .ok_or_else(|| anyhow!("memory scope required"))?
                .to_string(),
            category: args
                .get(2)
                .ok_or_else(|| anyhow!("memory category required"))?
                .to_string(),
            key: args
                .get(3)
                .ok_or_else(|| anyhow!("memory key required"))?
                .to_string(),
            value: args.iter().skip(4).copied().collect::<Vec<_>>().join(" "),
        },
        "memory" => Command::Memory {
            query: (!args.is_empty()).then(|| args.join(" ")),
        },
        "skills" if args.first() == Some(&"detail") => Command::SkillDetail {
            skill: args
                .get(1)
                .ok_or_else(|| anyhow!("skill id required"))?
                .to_string(),
        },
        "skills" if matches!(args.first(), Some(&"enable") | Some(&"disable")) => {
            Command::SetSkillEnabled {
                skill: args
                    .get(1)
                    .ok_or_else(|| anyhow!("skill id required"))?
                    .to_string(),
                enabled: args[0] == "enable",
            }
        }
        "skills" if args.first() == Some(&"delete-confirm") => Command::DeleteSkill {
            skill: args
                .get(1)
                .ok_or_else(|| anyhow!("skill id required"))?
                .to_string(),
        },
        "skills" if args.first() == Some(&"delete") => Command::ConfirmDeleteSkill {
            skill: args
                .get(1)
                .ok_or_else(|| anyhow!("skill id required"))?
                .to_string(),
        },
        "skills" => Command::Skills {
            query: (!args.is_empty()).then(|| args.join(" ")),
        },
        "tools" => Command::Tools,
        "about" => Command::About,
        "approvals" => Command::Approvals,
        "approve" => Command::Approve {
            request: args
                .first()
                .ok_or_else(|| anyhow!("approval request id required"))?
                .to_string(),
        },
        "deny" => Command::DenyApproval {
            request: args
                .first()
                .ok_or_else(|| anyhow!("approval request id required"))?
                .to_string(),
        },
        "doctor" => Command::Doctor,
        "help" => Command::Help { topic: one() },
        _ => return Err(anyhow!("unknown command /{name}")),
    }))
}

pub struct CommandCore {
    config: Arc<RwLock<AppConfig>>,
    storage: Arc<Storage>,
    sessions: Arc<SessionManager>,
    providers: Arc<ProviderRegistry>,
    auth: Arc<AuthManager>,
    health: Arc<HealthState>,
    events: Arc<EventBus>,
    agent: AgentEngine,
    runtime: Option<Arc<RuntimeState>>,
}

impl CommandCore {
    pub fn new(
        config: Arc<RwLock<AppConfig>>,
        storage: Arc<Storage>,
        sessions: Arc<SessionManager>,
        providers: Arc<ProviderRegistry>,
        auth: Arc<AuthManager>,
        health: Arc<HealthState>,
        events: Arc<EventBus>,
    ) -> Self {
        Self::build(
            config, storage, sessions, providers, auth, health, events, None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_runtime(
        config: Arc<RwLock<AppConfig>>,
        storage: Arc<Storage>,
        sessions: Arc<SessionManager>,
        providers: Arc<ProviderRegistry>,
        auth: Arc<AuthManager>,
        health: Arc<HealthState>,
        events: Arc<EventBus>,
        runtime: Arc<RuntimeState>,
    ) -> Self {
        Self::build(
            config,
            storage,
            sessions,
            providers,
            auth,
            health,
            events,
            Some(runtime),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        config: Arc<RwLock<AppConfig>>,
        storage: Arc<Storage>,
        sessions: Arc<SessionManager>,
        providers: Arc<ProviderRegistry>,
        auth: Arc<AuthManager>,
        health: Arc<HealthState>,
        events: Arc<EventBus>,
        runtime: Option<Arc<RuntimeState>>,
    ) -> Self {
        let agent_config = config
            .try_read()
            .map(|guard| guard.agent.clone())
            .unwrap_or_default();
        let agent = if let Some(runtime) = runtime.clone() {
            AgentEngine::with_runtime(
                sessions.clone(),
                storage.clone(),
                providers.clone(),
                agent_config,
                runtime,
            )
        } else {
            AgentEngine::with_config(
                sessions.clone(),
                storage.clone(),
                providers.clone(),
                agent_config,
            )
        };
        Self {
            config,
            storage,
            sessions,
            providers,
            auth,
            health,
            events,
            agent,
            runtime,
        }
    }

    pub async fn execute_text(&self, principal: &str, input: &str) -> Result<CommandResult> {
        self.execute_text_with_progress(principal, input, None)
            .await
    }

    pub async fn execute_text_with_progress(
        &self,
        principal: &str,
        input: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<CommandResult> {
        self.execute_text_with_context(principal, None, input, progress)
            .await
    }

    pub async fn execute_text_in_telegram_scope(
        &self,
        principal: &str,
        scope: TelegramScope,
        input: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<CommandResult> {
        self.execute_text_with_context(principal, Some(scope), input, progress)
            .await
    }

    async fn execute_text_with_context(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        input: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<CommandResult> {
        match parse(input)? {
            Some(cmd) => self.execute_in_scope(principal, scope, cmd).await,
            None => {
                if !self.config.read().await.gateway.enabled {
                    return Err(anyhow!("gateway is disabled"));
                }
                let answer = match scope {
                    Some(scope) => {
                        self.agent
                            .submit_with_progress_in_scope(principal, scope, input, progress)
                            .await?
                    }
                    None => {
                        self.agent
                            .submit_with_progress(principal, input, progress)
                            .await?
                    }
                };
                Ok(CommandResult::Agent(answer))
            }
        }
    }

    pub async fn retry_with_progress(
        &self,
        principal: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<CommandResult> {
        if !self.config.read().await.gateway.enabled {
            return Err(anyhow!("gateway is disabled"));
        }
        Ok(CommandResult::Agent(
            self.agent.retry_with_progress(principal, progress).await?,
        ))
    }

    pub async fn retry_with_progress_in_telegram_scope(
        &self,
        principal: &str,
        scope: TelegramScope,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<CommandResult> {
        if !self.config.read().await.gateway.enabled {
            return Err(anyhow!("gateway is disabled"));
        }
        Ok(CommandResult::Agent(
            self.agent
                .retry_with_progress_in_scope(principal, Some(scope), progress)
                .await?,
        ))
    }

    pub async fn execute(&self, principal: &str, command: Command) -> Result<CommandResult> {
        self.execute_in_scope(principal, None, command).await
    }

    pub async fn execute_in_scope(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        command: Command,
    ) -> Result<CommandResult> {
        use Command::*;
        match command {
            Start => Ok(CommandResult::InfoView(View {
                title: None,
                blocks: vec![Block::Paragraph {
                    text: "Hello! I'm Xiao. How can I help you today?".into(),
                }],
                actions: Vec::new(),
                side_mode: false,
            })),
            NewSession => {
                let s = match scope {
                    Some(scope) => self.sessions.create_and_switch_telegram(principal, scope)?,
                    None => self.sessions.create_and_switch(principal)?,
                };
                self.events.publish(AppEvent::SessionChanged {
                    principal: principal.into(),
                    session_id: s.id.clone(),
                });
                let mut view =
                    View::info("NEW SESSION", format!("Created and activated {}", s.name));
                view.actions = vec![vec![
                    Action::command("Session Manager", "/sessions"),
                    Action::close(),
                ]];
                Ok(CommandResult::Confirmation(view))
            }
            ToggleSideChat => {
                let c = match scope {
                    Some(scope) => self.sessions.toggle_telegram_side(principal, scope)?,
                    None => self.sessions.toggle_side(principal)?,
                };
                let text = if c.mode == ChatMode::Side {
                    format!("SIDE CHAT SESSION\nParent: {}", c.main.name)
                } else {
                    format!("Returned to MAIN SESSION\n{}", c.main.name)
                };
                Ok(CommandResult::Confirmation(View::info(
                    "SESSION MODE",
                    text,
                )))
            }
            Session { page } => Ok(CommandResult::ManagerView(
                self.session_view(principal, scope, page)?,
            )),
            SessionDetail { session } => Ok(CommandResult::ManagerView(
                self.session_detail_view(principal, scope, &session)?,
            )),
            SwitchSession { session } => {
                let s = match scope {
                    Some(scope) => self
                        .sessions
                        .switch_telegram_main(principal, scope, &session)?,
                    None => self.sessions.switch_main(principal, &session)?,
                };
                self.events.publish(AppEvent::SessionChanged {
                    principal: principal.into(),
                    session_id: s.id.clone(),
                });
                Ok(CommandResult::ManagerView(self.session_view_with_notice(
                    principal,
                    scope,
                    1,
                    format!("Active: {}", s.name),
                )?))
            }
            RequestRenameSession { session } => {
                self.validate_session_scope(principal, scope, &session)?;
                let target = self
                    .storage
                    .session(principal, &session)?
                    .ok_or_else(|| anyhow!("session not found"))?;
                if target.is_side || target.archived {
                    return Err(anyhow!("session is not renameable"));
                }
                let view = View {
                    title: Some("RENAME SESSION".into()),
                    blocks: vec![Block::Paragraph {
                        text: format!(
                            "Current name: {}\nSend the new name as your next message.",
                            target.name
                        ),
                    }],
                    actions: vec![vec![Action::back(), Action::close()]],
                    side_mode: false,
                };
                Ok(CommandResult::InputRequest {
                    view,
                    command_prefix: format!("/sessions rename {}", target.id),
                })
            }
            RenameSession { session, name } => {
                self.validate_session_scope(principal, scope, &session)?;
                let name = name.trim();
                if name.is_empty() {
                    return Err(anyhow!("new session name required"));
                }
                if name.chars().count() > 120 {
                    return Err(anyhow!("session name must be 120 characters or fewer"));
                }
                self.storage.rename_session(principal, &session, name)?;
                Ok(CommandResult::ManagerView(
                    self.session_detail_view(principal, scope, &session)?,
                ))
            }
            ArchiveSession { session } => {
                let active = match scope {
                    Some(scope) => self
                        .sessions
                        .archive_and_recover_telegram(principal, scope, &session)?,
                    None => self.sessions.archive_and_recover(principal, &session)?,
                };
                self.events.publish(AppEvent::SessionArchived {
                    principal: principal.into(),
                    session_id: session,
                });
                self.events.publish(AppEvent::SessionChanged {
                    principal: principal.into(),
                    session_id: active.id.clone(),
                });
                Ok(CommandResult::ManagerView(self.session_view_with_notice(
                    principal,
                    scope,
                    1,
                    format!("Archived. Active: {}", active.name),
                )?))
            }
            Model => Ok(CommandResult::ManagerView(
                self.model_view(principal, scope)?,
            )),
            ModelPicker { page } => Ok(CommandResult::ManagerView(
                self.model_picker_view(principal, scope, page)?,
            )),
            SetModel { model } => {
                let custom_configured = {
                    let config = self.config.read().await;
                    config.providers.custom.enabled && config.providers.custom.base_url.is_some()
                };
                if custom_configured
                    && self.session_context(principal, scope)?.active.provider == "custom"
                {
                    self.probe_custom_model(principal, scope, &model).await?;
                }
                self.set_model(principal, scope, &model)?;
                self.events.publish(AppEvent::ModelChanged {
                    principal: principal.into(),
                    model,
                });
                Ok(CommandResult::ManagerView(
                    self.model_view(principal, scope)?,
                ))
            }
            Account => Ok(CommandResult::ManagerView(
                self.account_view(principal, scope)?,
            )),
            UseAccount { account } => {
                let (provider, model) = self.use_account(principal, scope, &account)?;
                self.events.publish(AppEvent::ProviderChanged {
                    principal: principal.into(),
                    provider: provider.clone(),
                });
                self.events.publish(AppEvent::AccountChanged {
                    principal: principal.into(),
                    account_id: Some(account.clone()),
                });
                self.events.publish(AppEvent::ModelChanged {
                    principal: principal.into(),
                    model: model.clone(),
                });
                Ok(CommandResult::ManagerView(
                    self.model_view(principal, scope)?,
                ))
            }
            Login { provider: None } => Ok(CommandResult::ManagerView(login_picker())),
            Login {
                provider: Some(provider),
            } => {
                let provider = normalize_provider(&provider);
                if provider == "custom" {
                    return Ok(CommandResult::StartCustomLogin);
                }
                let challenge = self.auth.begin_login(&provider).await?;
                let transaction_id = match &challenge {
                    AuthChallenge::BrowserUrl { transaction_id, .. } => transaction_id.clone(),
                    AuthChallenge::ApiKey { .. } => "local-api-key".into(),
                };
                self.events.publish(AppEvent::AuthStarted {
                    provider,
                    transaction_id,
                });
                Ok(CommandResult::StartedAuth(challenge))
            }
            CancelAuth { transaction } => {
                self.auth.cancel_transaction(&transaction);
                let mut view = View::info("LOGIN", "Authentication cancelled");
                view.actions = vec![vec![Action::command("Login", "/login"), Action::close()]];
                Ok(CommandResult::Confirmation(view))
            }
            Logout { account } => {
                let id = match account {
                    Some(v) => v,
                    None => self
                        .sessions
                        .context_for(principal)?
                        .active
                        .account_id
                        .ok_or_else(|| anyhow!("no active account"))?,
                };
                self.auth.logout(&id)?;
                Ok(CommandResult::Confirmation(View::info(
                    "ACCOUNT",
                    "Disconnected",
                )))
            }
            Status => Ok(CommandResult::InfoView(
                self.status_view(principal, scope).await?,
            )),
            Context => Ok(CommandResult::InfoView(
                self.context_view(principal, scope).await?,
            )),
            Stop => Ok(CommandResult::Confirmation(View::info(
                "CANCEL",
                if self.agent.cancel_in_scope(principal, scope) {
                    "Cancellation requested"
                } else {
                    "No active generation"
                },
            ))),
            Retry => {
                let answer = self
                    .agent
                    .retry_with_progress_in_scope(principal, scope, None)
                    .await?;
                Ok(CommandResult::Agent(answer))
            }
            Yolo { enabled } => {
                let context = self.session_context(principal, scope)?;
                if let Some(enabled) = enabled {
                    self.storage
                        .set_session_yolo(principal, &context.active.id, enabled)?;
                    self.storage.audit(
                        principal,
                        if enabled {
                            "yolo_enabled"
                        } else {
                            "yolo_disabled"
                        },
                        &format!("session_id={}", context.active.id),
                    )?;
                }
                Ok(CommandResult::ManagerView(
                    self.yolo_view(principal, scope)?,
                ))
            }
            Memory { query } if query.as_deref() == Some("search") => {
                Ok(CommandResult::InputRequest {
                    view: View::info("MEMORY SEARCH", "Send the memory search query."),
                    command_prefix: "/memory".into(),
                })
            }
            Memory { query } => Ok(CommandResult::ManagerView(
                self.memory_view(principal, query.as_deref())?,
            )),
            RequestMemoryEdit {
                scope: memory_scope,
                category,
                key,
            } => {
                let store = self.memory_store();
                let scope = MemoryScope::try_from(memory_scope.as_str())?;
                let record = store
                    .list(principal, Some(scope), 500)?
                    .into_iter()
                    .find(|record| record.category == category && record.key == key)
                    .ok_or_else(|| anyhow!("memory entry not found"))?;
                Ok(CommandResult::InputRequest {
                    view: View::info(
                        "EDIT MEMORY",
                        format!(
                            "Current value: {}\nSend the replacement value.",
                            record.value
                        ),
                    ),
                    command_prefix: format!("/memory edit {memory_scope} {category} {key}"),
                })
            }
            EditMemory {
                scope: memory_scope,
                category,
                key,
                value,
            } => {
                let value = value.trim();
                if value.is_empty() {
                    return Err(anyhow!("memory value is required"));
                }
                let scope = MemoryScope::try_from(memory_scope.as_str())?;
                self.memory_store().upsert(
                    principal,
                    scope,
                    &category,
                    &key,
                    value,
                    1.0,
                    "owner_telegram_edit",
                    None,
                )?;
                Ok(CommandResult::ManagerView(
                    self.memory_view(principal, Some(scope.as_str()))?,
                ))
            }
            ForgetMemory {
                scope: memory_scope,
                category,
                key,
            } => {
                let memory_scope = MemoryScope::try_from(memory_scope.as_str())?;
                self.memory_store()
                    .delete(principal, memory_scope, &category, &key, None)?;
                Ok(CommandResult::ManagerView(
                    self.memory_view(principal, Some(memory_scope.as_str()))?,
                ))
            }
            Skills { query } if query.as_deref() == Some("search") => {
                Ok(CommandResult::InputRequest {
                    view: View::info("SKILL SEARCH", "Send the skill search query."),
                    command_prefix: "/skills".into(),
                })
            }
            Skills { query } => Ok(CommandResult::ManagerView(
                self.skills_view(principal, query.as_deref())?,
            )),
            SkillDetail { skill } => Ok(CommandResult::ManagerView(
                self.skill_detail_view(principal, &skill)?,
            )),
            SetSkillEnabled { skill, enabled } => {
                SkillStore::new(self.storage.clone()).set_enabled(principal, &skill, enabled)?;
                self.storage.audit(
                    principal,
                    if enabled {
                        "skill_enabled"
                    } else {
                        "skill_disabled"
                    },
                    &format!("skill_id={skill}"),
                )?;
                Ok(CommandResult::ManagerView(
                    self.skill_detail_view(principal, &skill)?,
                ))
            }
            ConfirmDeleteSkill { skill } => {
                let record = SkillStore::new(self.storage.clone())
                    .view(principal, &skill)?
                    .ok_or_else(|| anyhow!("skill not found"))?;
                if record.source_kind != "learned" {
                    return Err(anyhow!("only learned owner-created skills can be deleted"));
                }
                let mut view = View::info(
                    "DELETE LEARNED SKILL",
                    format!(
                        "Delete {}? This removes its active index and owner-created SKILL.md. Audit history remains.",
                        record.name
                    ),
                );
                view.actions = vec![
                    vec![Action::command(
                        "Delete",
                        format!("/skills delete-confirm {}", record.id),
                    )],
                    vec![Action::back(), Action::close()],
                ];
                Ok(CommandResult::ManagerView(view))
            }
            DeleteSkill { skill } => {
                let store = SkillStore::new(self.storage.clone());
                let record = store
                    .view(principal, &skill)?
                    .ok_or_else(|| anyhow!("skill not found"))?;
                if record.source_kind != "learned" {
                    return Err(anyhow!("only learned owner-created skills can be deleted"));
                }
                if let Some(runtime) = &self.runtime {
                    runtime.workspace().delete_learned_skill(&record.name)?;
                }
                store.delete_learned(principal, &record.id)?;
                self.storage.audit(
                    principal,
                    "learned_skill_deleted",
                    &format!("skill_id={};name={}", record.id, record.name),
                )?;
                Ok(CommandResult::ManagerView(
                    self.skills_view(principal, Some("learned"))?,
                ))
            }
            Tools => Ok(CommandResult::ManagerView(self.tools_view())),
            About => Ok(CommandResult::InfoView(self.about_view())),
            Approvals => {
                let approvals = self.storage.pending_approvals(principal)?;
                let body = if approvals.is_empty() {
                    "No pending sensitive or privileged operations.".into()
                } else {
                    approvals
                        .iter()
                        .map(|approval| {
                            format!(
                                "{}\n{} · {}\nExpires: {}",
                                approval.id,
                                approval.tool_name,
                                approval.summary,
                                approval.expires_at
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                };
                let mut view = View::info("APPROVALS", body);
                view.actions = approvals
                    .into_iter()
                    .take(5)
                    .map(|approval| {
                        vec![
                            Action::command("Approve", format!("/approve {}", approval.id)),
                            Action::command("Deny", format!("/deny {}", approval.id)),
                        ]
                    })
                    .collect();
                Ok(CommandResult::ManagerView(view))
            }
            Approve { request } => {
                if !self.storage.decide_approval(principal, &request, true)? {
                    return Err(anyhow!("pending approval request not found or expired"));
                }
                Ok(CommandResult::Confirmation(View::info(
                    "APPROVED",
                    "One exact operation was approved. Use /retry to resume the original request; the grant is consumed once.",
                )))
            }
            DenyApproval { request } => {
                if !self.storage.decide_approval(principal, &request, false)? {
                    return Err(anyhow!("pending approval request not found or expired"));
                }
                Ok(CommandResult::Confirmation(View::info(
                    "DENIED",
                    "The pending sensitive operation was denied.",
                )))
            }
            Help { .. } => Ok(CommandResult::InfoView(help_view())),
            Doctor => Ok(CommandResult::InfoView(self.doctor_view().await)),
        }
    }

    fn session_view(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        page: usize,
    ) -> Result<View> {
        self.session_view_with_notice(principal, scope, page, String::new())
    }

    fn session_view_with_notice(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        page: usize,
        notice: String,
    ) -> Result<View> {
        let (rows, pages, page) = match scope {
            Some(scope) => self
                .sessions
                .list_telegram_page(principal, scope, page, 5)?,
            None => self.sessions.list_page(principal, page, 5)?,
        };
        let active = self.session_context(principal, scope)?.main.id;
        let mut blocks = vec![];
        if !notice.is_empty() {
            blocks.push(Block::Paragraph { text: notice });
        }
        blocks.push(Block::Table {
            headers: vec![
                "No".into(),
                "Active".into(),
                "Session name".into(),
                "Messages".into(),
                "Last activity".into(),
            ],
            rows: rows
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    vec![
                        (i + 1).to_string(),
                        if s.id == active {
                            "●".into()
                        } else {
                            "".into()
                        },
                        s.name.clone(),
                        s.message_count.to_string(),
                        short_activity(&s.last_active_at),
                    ]
                })
                .collect(),
        });
        let mut actions = vec![];
        if !rows.is_empty() {
            actions.push(
                rows.iter()
                    .enumerate()
                    .map(|(i, s)| {
                        Action::command((i + 1).to_string(), format!("/sessions switch {}", s.id))
                    })
                    .collect(),
            );
        }
        actions.push(vec![
            Action::command("‹", format!("/sessions {}", page.saturating_sub(1).max(1))),
            Action::noop(format!("{page}/{pages}")),
            Action::command("›", format!("/sessions {}", (page + 1).min(pages))),
        ]);
        actions.push(vec![
            Action::command("New", "/new"),
            Action::command("Rename", format!("/sessions rename {active}")),
            Action::command("Detail", format!("/sessions detail {active}")),
        ]);
        actions.push(vec![
            Action::command("Archive", format!("/sessions archive {active}")),
            Action::close(),
        ]);
        Ok(View {
            title: Some("SESSIONS".into()),
            blocks,
            actions,
            side_mode: false,
        })
    }

    fn session_detail_view(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        id: &str,
    ) -> Result<View> {
        let s = self
            .storage
            .session(principal, id)?
            .ok_or_else(|| anyhow!("session not found"))?;
        if s.is_side {
            return Err(anyhow!(
                "side sessions are not exposed in the main session manager"
            ));
        }
        if let Some(scope) = scope {
            let bound = self.storage.telegram_scope_for_session(principal, id)?;
            if bound != Some((scope.chat_id, scope.message_thread_id)) {
                return Err(anyhow!("session is not in this Telegram topic"));
            }
        }
        let context = self.session_context(principal, scope)?;
        let active = context.main.id == s.id;
        let mode = if active {
            context.mode.as_str()
        } else {
            "main"
        };
        let rows = vec![
            vec!["NAME".into(), s.name.clone()],
            vec!["ID".into(), s.id.clone()],
            vec!["PROVIDER".into(), s.provider.clone()],
            vec![
                "ACCOUNT".into(),
                s.account_id.clone().unwrap_or_else(|| "—".into()),
            ],
            vec!["MODEL".into(), s.model.clone()],
            vec!["MESSAGES".into(), s.message_count.to_string()],
            vec![
                "CONTEXT".into(),
                format!("{} stored messages", s.message_count),
            ],
            vec!["MODE".into(), mode.into()],
            vec![
                "STATUS".into(),
                if s.archived {
                    "ARCHIVED".into()
                } else if active {
                    "ACTIVE".into()
                } else {
                    "READY".into()
                },
            ],
        ];
        let mut actions = vec![vec![
            Action::command("Select", format!("/sessions switch {}", s.id)),
            Action::command("Rename", format!("/sessions rename {}", s.id)),
        ]];
        actions.push(vec![
            Action::command("Archive", format!("/sessions archive {}", s.id)),
            Action::back(),
            Action::close(),
        ]);
        Ok(View {
            title: Some("SESSION DETAIL".into()),
            blocks: vec![Block::Table {
                headers: vec!["Field".into(), "Value".into()],
                rows,
            }],
            actions,
            side_mode: false,
        })
    }

    fn model_view(&self, principal: &str, scope: Option<TelegramScope>) -> Result<View> {
        let c = self.session_context(principal, scope)?;
        let account = c
            .active
            .account_id
            .as_deref()
            .and_then(|id| self.storage.account(id).ok().flatten())
            .map(|a| account_label(&a))
            .unwrap_or_else(|| "—".into());
        let status = format!("{:?}", self.providers.state(&c.active.provider)).to_uppercase();
        let protocol = self
            .providers
            .capabilities(&c.active.provider, &c.active.model)
            .map(|capability| capability.tool_protocol.as_str().to_owned())
            .unwrap_or_else(|_| "unknown".into());
        Ok(View {
            title: Some("MODEL".into()),
            blocks: vec![Block::Table {
                headers: vec!["Field".into(), "Value".into()],
                rows: vec![
                    vec!["PROVIDER".into(), c.active.provider.clone()],
                    vec!["ACCOUNT".into(), account],
                    vec!["MODEL".into(), c.active.model.clone()],
                    vec!["AGENT PROTOCOL".into(), protocol],
                    vec!["REASONING".into(), "Provider default".into()],
                    vec!["SESSION".into(), c.main.name],
                    vec!["STATUS".into(), status],
                ],
            }],
            actions: vec![
                vec![Action::command("Change Model", "/model change")],
                vec![
                    Action::command("Account", "/account"),
                    Action::command("Login", "/login"),
                ],
                vec![Action::close()],
            ],
            side_mode: c.mode == ChatMode::Side,
        })
    }

    fn model_picker_view(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        page: usize,
    ) -> Result<View> {
        let c = self.session_context(principal, scope)?;
        let models = self.providers.models(&c.active.provider)?;
        let pages = models.len().max(1).div_ceil(5);
        let page = page.clamp(1, pages);
        let visible = models
            .iter()
            .skip((page - 1) * 5)
            .take(5)
            .cloned()
            .collect::<Vec<_>>();
        let rows = visible
            .iter()
            .enumerate()
            .map(|(index, m)| {
                vec![
                    (index + 1).to_string(),
                    m.clone(),
                    if *m == c.active.model {
                        "●".into()
                    } else {
                        "".into()
                    },
                ]
            })
            .collect();
        let mut actions = vec![visible
            .into_iter()
            .enumerate()
            .map(|(index, model)| {
                Action::command((index + 1).to_string(), format!("/model {model}"))
            })
            .collect::<Vec<_>>()];
        actions.push(vec![
            Action::command(
                "‹",
                format!("/model change {}", page.saturating_sub(1).max(1)),
            ),
            Action::noop(format!("Page {page}/{pages}")),
            Action::command("›", format!("/model change {}", (page + 1).min(pages))),
        ]);
        actions.push(vec![Action::back(), Action::close()]);
        Ok(View {
            title: Some("MODEL".into()),
            blocks: vec![Block::Table {
                headers: vec!["No".into(), "Model".into(), "Current".into()],
                rows,
            }],
            actions,
            side_mode: false,
        })
    }

    fn account_view(&self, principal: &str, scope: Option<TelegramScope>) -> Result<View> {
        let c = self.session_context(principal, scope)?;
        let accounts = self.auth.accounts(Some(&c.active.provider))?;
        let rows = accounts
            .iter()
            .map(|a| {
                vec![
                    if c.active.account_id.as_deref() == Some(a.id.as_str()) {
                        "●".into()
                    } else {
                        "".into()
                    },
                    a.label.clone(),
                    a.status.clone(),
                ]
            })
            .collect();
        let mut actions = accounts
            .into_iter()
            .map(|a| {
                vec![Action::command(
                    a.label.clone(),
                    format!("/account {}", a.id),
                )]
            })
            .collect::<Vec<_>>();
        actions.push(vec![
            Action::command("Add / Login", format!("/login {}", c.active.provider)),
            Action::close(),
        ]);
        Ok(View {
            title: Some("ACCOUNT".into()),
            blocks: vec![Block::Table {
                headers: vec!["Current".into(), "Account".into(), "Status".into()],
                rows,
            }],
            actions,
            side_mode: false,
        })
    }

    fn set_model(&self, principal: &str, scope: Option<TelegramScope>, model: &str) -> Result<()> {
        let c = self.session_context(principal, scope)?;
        if !self
            .providers
            .models(&c.active.provider)?
            .iter()
            .any(|m| m == model)
        {
            return Err(anyhow!("model is not in the provider catalog"));
        }
        self.storage.set_session_provider(
            principal,
            &c.active.id,
            &c.active.provider,
            c.active.account_id.as_deref(),
            model,
        )
    }

    async fn probe_custom_model(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        model: &str,
    ) -> Result<()> {
        let context = self.session_context(principal, scope)?;
        if !self
            .providers
            .models("custom")?
            .iter()
            .any(|candidate| candidate == model)
        {
            return Err(anyhow!("model is not in the provider catalog"));
        }
        let custom = self.config.read().await.providers.custom.clone();
        let endpoint = custom
            .base_url
            .as_deref()
            .ok_or_else(|| anyhow!("custom endpoint is not configured"))?;
        let selected_api_key = context
            .active
            .account_id
            .as_deref()
            .and_then(|account| self.auth.credential(account).ok().flatten())
            .and_then(|credential| credential.api_key);
        let fallback_api_key = if selected_api_key.is_none() {
            self.auth.provider_api_key("custom")?
        } else {
            None
        };
        let capability = crate::providers::probe_custom_tool_capability(
            endpoint,
            &custom.headers,
            selected_api_key.as_deref().or(fallback_api_key.as_deref()),
            &custom.protocol,
            model,
        )
        .await;
        self.storage
            .upsert_provider_capability(&crate::storage::ProviderCapabilityRecord {
                provider: "custom".into(),
                model: model.into(),
                tool_protocol: capability.tool_protocol.as_str().into(),
                native_tool_calls: capability.tool_protocol
                    == crate::providers::ToolProtocol::Native,
                structured_output: capability.structured_output,
                continuation: capability.continuation,
                probed_at: chrono::Utc::now().to_rfc3339(),
                evidence: capability.evidence,
            })?;
        Ok(())
    }

    /// Atomic provider/account/model activation. Provider and model are resolved before
    /// the transaction; the storage layer then updates all three fields together.
    fn use_account(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        account: &str,
    ) -> Result<(String, String)> {
        let record = self
            .storage
            .account(account)?
            .ok_or_else(|| anyhow!("account not found"))?;
        if record.status != "connected" {
            return Err(anyhow!("account is not connected"));
        }
        let model = self.providers.preferred_model(&record.provider)?;
        let c = self.session_context(principal, scope)?;
        self.storage.activate_account(
            principal,
            &c.active.id,
            account,
            &record.provider,
            &model,
        )?;
        Ok((record.provider, model))
    }

    fn session_context(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
    ) -> Result<crate::session::SessionContext> {
        match scope {
            Some(scope) => self.sessions.context_for_telegram(principal, scope),
            None => self.sessions.context_for(principal),
        }
    }

    fn validate_session_scope(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        session_id: &str,
    ) -> Result<()> {
        if let Some(scope) = scope {
            let bound = self
                .storage
                .telegram_scope_for_session(principal, session_id)?;
            if bound != Some((scope.chat_id, scope.message_thread_id)) {
                return Err(anyhow!("session is not in this Telegram topic"));
            }
        }
        Ok(())
    }

    async fn status_view(&self, principal: &str, scope: Option<TelegramScope>) -> Result<View> {
        let cfg = self.config.read().await.clone();
        let health = self
            .health
            .snapshot(&cfg, self.storage.health(), self.providers.states())
            .await;
        let c = self.session_context(principal, scope)?;
        let selected_state = health
            .provider_states
            .get(&c.active.provider)
            .cloned()
            .unwrap_or(ProviderState::Error);
        let provider_state = format!("{selected_state:?}").to_uppercase();
        // `/status` is principal/session aware. A globally healthy daemon is still a
        // degraded gateway for this principal when the selected provider cannot serve
        // requests (expired credential, missing login/configuration, etc.).
        let gateway = if matches!(&health.gateway, GatewayStatus::Running)
            && selected_state != ProviderState::Ready
        {
            GatewayStatus::Degraded
        } else {
            health.gateway.clone()
        };
        let environment = self.runtime.as_ref().map(|runtime| runtime.environment());
        let skills = SkillStore::new(self.storage.clone())
            .list(principal, 500)?
            .len();
        let latest_run = self
            .storage
            .agent_runs(principal, 20)?
            .into_iter()
            .find(|run| run.session_id == c.active.id && run.status == "running");
        Ok(View {
            title: Some("STATUS".into()),
            blocks: vec![Block::Table {
                headers: vec!["Field".into(), "Value".into()],
                rows: vec![
                    vec![
                        "Agent".into(),
                        if self.agent.is_active_in_scope(principal, scope) {
                            "RUNNING".into()
                        } else if matches!(gateway, GatewayStatus::Running) {
                            "READY".into()
                        } else {
                            format!("{:?}", gateway).to_uppercase()
                        },
                    ],
                    vec![
                        "Daemon".into(),
                        if health.daemon_running {
                            "RUNNING".into()
                        } else {
                            "STOPPED".into()
                        },
                    ],
                    vec![
                        "Telegram".into(),
                        if health.telegram_polling {
                            "POLLING".into()
                        } else {
                            "IDLE".into()
                        },
                    ],
                    vec![
                        "DB".into(),
                        if health.db_healthy {
                            "HEALTHY".into()
                        } else {
                            "ERROR".into()
                        },
                    ],
                    vec![
                        "Provider".into(),
                        format!("{} · {}", c.active.provider, provider_state),
                    ],
                    vec!["Model".into(), c.active.model],
                    vec!["Session".into(), c.main.name],
                    vec![
                        "Topic".into(),
                        scope
                            .and_then(|scope| scope.message_thread_id)
                            .map(|thread| thread.to_string())
                            .unwrap_or_else(|| "default".into()),
                    ],
                    vec![
                        "YOLO".into(),
                        if c.active.yolo_mode {
                            "ON".into()
                        } else {
                            "OFF".into()
                        },
                    ],
                    vec![
                        "Termux".into(),
                        if environment.as_ref().is_some_and(|env| env.termux.is_some()) {
                            "READY".into()
                        } else {
                            "UNAVAILABLE".into()
                        },
                    ],
                    vec![
                        "Root broker".into(),
                        if environment
                            .as_ref()
                            .is_some_and(|env| env.effective_uid == 0)
                        {
                            "READY".into()
                        } else {
                            "UNAVAILABLE".into()
                        },
                    ],
                    vec!["Memory".into(), "READY".into()],
                    vec!["Skills".into(), skills.to_string()],
                    vec![
                        "Running task".into(),
                        latest_run
                            .and_then(|run| run.goal)
                            .unwrap_or_else(|| "—".into()),
                    ],
                ],
            }],
            actions: vec![],
            side_mode: c.mode == ChatMode::Side,
        })
    }

    async fn context_view(&self, principal: &str, scope: Option<TelegramScope>) -> Result<View> {
        let c = self.session_context(principal, scope)?;
        let main = self.storage.messages(principal, &c.main.id)?;
        let side = if c.mode == ChatMode::Side {
            self.storage.messages(principal, &c.active.id)?
        } else {
            Vec::new()
        };
        let effective_count = main.len() + side.len();
        let chars = main
            .iter()
            .chain(side.iter())
            .map(|message| message.content.chars().count())
            .sum::<usize>();
        let memories = MemoryStore::new(self.storage.clone())
            .list(principal, None, 200)?
            .len();
        let skills = SkillStore::new(self.storage.clone())
            .list(principal, 500)?
            .len();
        let summarized = self
            .storage
            .session_summary(principal, &c.main.id)?
            .is_some();
        let context_budget = self.config.read().await.agent.context_max_chars;
        Ok(View {
            title: Some("CONTEXT".into()),
            blocks: vec![Block::Table {
                headers: vec!["Field".into(), "Value".into()],
                rows: vec![
                    vec!["Main messages".into(), c.main.message_count.to_string()],
                    vec!["Effective messages".into(), effective_count.to_string()],
                    vec!["Stored characters".into(), chars.to_string()],
                    vec!["Context budget".into(), format!("{context_budget} chars")],
                    vec![
                        "Summary".into(),
                        if summarized {
                            "AVAILABLE".into()
                        } else {
                            "NOT NEEDED".into()
                        },
                    ],
                    vec!["Active memory entries".into(), memories.to_string()],
                    vec!["Skills available".into(), skills.to_string()],
                    vec![
                        "Provider / model".into(),
                        format!("{} / {}", c.active.provider, c.active.model),
                    ],
                    vec!["Mode".into(), c.mode.as_str().to_uppercase()],
                    vec![
                        "Side isolation".into(),
                        if c.mode == ChatMode::Side {
                            "READ MAIN + WRITE SIDE".into()
                        } else {
                            "MAIN".into()
                        },
                    ],
                ],
            }],
            actions: vec![],
            side_mode: c.mode == ChatMode::Side,
        })
    }

    async fn doctor_view(&self) -> View {
        let cfg = self.config.read().await.clone();
        let environment = self.runtime.as_ref().map(|runtime| runtime.environment());
        let identity_ready = self
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.workspace().load().is_ok());
        let checks = vec![
            format!("DB: {}", if self.storage.health() { "OK" } else { "ERROR" }),
            format!("IPC: {}", cfg.ipc.bind),
            format!("Telegram transport: {}", cfg.telegram.transport),
            format!("Providers registered: {}", self.providers.readiness()),
            format!(
                "Identity workspace: {}",
                if identity_ready { "OK" } else { "ERROR" }
            ),
            format!(
                "Memory index: {}",
                if self.storage.health() { "OK" } else { "ERROR" }
            ),
            format!(
                "Skills index: {}",
                if self.storage.health() { "OK" } else { "ERROR" }
            ),
            format!(
                "Termux: {}",
                if environment.as_ref().is_some_and(|env| env.termux.is_some()) {
                    "OK"
                } else {
                    "UNAVAILABLE"
                }
            ),
            format!(
                "Privileged broker: {}",
                if environment
                    .as_ref()
                    .is_some_and(|env| env.effective_uid == 0)
                {
                    "OK"
                } else {
                    "UNAVAILABLE"
                }
            ),
            "Root AI shell: DISABLED by architecture".into(),
        ];
        View {
            title: Some("DOCTOR".into()),
            blocks: vec![Block::List {
                ordered: false,
                items: checks,
            }],
            actions: vec![],
            side_mode: false,
        }
    }

    fn yolo_view(&self, principal: &str, scope: Option<TelegramScope>) -> Result<View> {
        let context = self.session_context(principal, scope)?;
        let enabled = context.active.yolo_mode;
        Ok(View {
            title: Some("YOLO MODE".into()),
            blocks: vec![Block::Paragraph {
                text: format!(
                    "Current session: {}\n\nYOLO skips interactive approvals that Xiao policy normally marks ASK. Hard DENY rules still apply.",
                    if enabled { "ON" } else { "OFF" }
                ),
            }],
            actions: vec![
                vec![Action::command(
                    if enabled { "Disable" } else { "Enable" },
                    if enabled { "/yolo disable" } else { "/yolo enable" },
                )],
                vec![Action::close()],
            ],
            side_mode: context.mode == ChatMode::Side,
        })
    }

    fn memory_view(&self, principal: &str, query: Option<&str>) -> Result<View> {
        let store = self.memory_store();
        let records = match query {
            Some("user") => store.list(principal, Some(MemoryScope::User), 5)?,
            Some("agent") => store.list(principal, Some(MemoryScope::Agent), 5)?,
            Some(query) if !query.trim().is_empty() => store.search(principal, query, 5)?,
            _ => Vec::new(),
        };
        let blocks = if records.is_empty() {
            vec![Block::Paragraph {
                text: "Choose a memory source or search Xiao's current inspectable state.".into(),
            }]
        } else {
            vec![Block::Table {
                headers: vec!["Scope".into(), "Key".into(), "Value".into()],
                rows: records
                    .iter()
                    .map(|record| {
                        vec![
                            record.scope.as_str().into(),
                            record.key.clone(),
                            record.value.clone(),
                        ]
                    })
                    .collect(),
            }]
        };
        Ok(View {
            title: Some("MEMORY".into()),
            blocks,
            actions: {
                let mut actions = vec![
                    vec![
                        Action::command("User Profile", "/memory user"),
                        Action::command("Agent Memory", "/memory agent"),
                    ],
                    vec![Action::command("Search", "/memory search"), Action::close()],
                ];
                for record in records.iter().take(5) {
                    actions.push(vec![
                        Action::command(
                            format!("Edit {}", record.key),
                            format!(
                                "/memory edit {} {} {}",
                                record.scope.as_str(),
                                record.category,
                                record.key
                            ),
                        ),
                        Action::command(
                            "Forget",
                            format!(
                                "/memory forget {} {} {}",
                                record.scope.as_str(),
                                record.category,
                                record.key
                            ),
                        ),
                    ]);
                }
                actions
            },
            side_mode: false,
        })
    }

    fn memory_store(&self) -> MemoryStore {
        match &self.runtime {
            Some(runtime) => MemoryStore::with_workspace(self.storage.clone(), runtime.workspace()),
            None => MemoryStore::new(self.storage.clone()),
        }
    }

    fn skills_view(&self, principal: &str, query: Option<&str>) -> Result<View> {
        let store = SkillStore::new(self.storage.clone());
        let query_parts = query
            .map(str::split_whitespace)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let source = query_parts
            .first()
            .copied()
            .filter(|source| matches!(*source, "learned" | "imported"));
        let requested_page = query_parts
            .get(1)
            .and_then(|page| page.parse::<usize>().ok())
            .unwrap_or(1);
        let mut pages = 1usize;
        let mut page = 1usize;
        let skills = match (source, query) {
            (Some(source), _) => {
                let matching = store
                    .list_all(principal, 500)?
                    .into_iter()
                    .filter(|skill| skill.source_kind == source)
                    .collect::<Vec<_>>();
                pages = matching.len().max(1).div_ceil(5);
                page = requested_page.clamp(1, pages);
                matching.into_iter().skip((page - 1) * 5).take(5).collect()
            }
            (None, Some(query)) if !query.trim().is_empty() => store.search(principal, query, 5)?,
            _ => Vec::new(),
        };
        let mut actions = vec![
            vec![
                Action::command("Learned", "/skills learned"),
                Action::command("Imported", "/skills imported"),
            ],
            vec![Action::command("Search", "/skills search"), Action::close()],
        ];
        for skill in skills.iter().take(5) {
            actions.push(vec![Action::command(
                format!("View {}", skill.name),
                format!("/skills detail {}", skill.id),
            )]);
        }
        if let Some(source) = source {
            actions.push(vec![
                Action::command(
                    "‹",
                    format!("/skills {source} {}", page.saturating_sub(1).max(1)),
                ),
                Action::noop(format!("Page {page}/{pages}")),
                Action::command("›", format!("/skills {source} {}", (page + 1).min(pages))),
            ]);
        }
        Ok(View {
            title: Some("SKILLS".into()),
            blocks: if skills.is_empty() {
                vec![Block::Paragraph {
                    text: "No matching indexed skills.".into(),
                }]
            } else {
                vec![Block::Table {
                    headers: vec!["No".into(), "Skill".into(), "Description".into()],
                    rows: skills
                        .iter()
                        .enumerate()
                        .map(|(index, skill)| {
                            vec![
                                (index + 1).to_string(),
                                skill.name.clone(),
                                format!(
                                    "{}{}",
                                    skill.summary,
                                    if skill.enabled { "" } else { " · DISABLED" }
                                ),
                            ]
                        })
                        .collect(),
                }]
            },
            actions,
            side_mode: false,
        })
    }

    fn skill_detail_view(&self, principal: &str, skill: &str) -> Result<View> {
        let record = SkillStore::new(self.storage.clone())
            .view(principal, skill)?
            .ok_or_else(|| anyhow!("skill not found"))?;
        let prerequisites = if record.prerequisites.trim().is_empty() {
            "None declared".into()
        } else {
            record.prerequisites.clone()
        };
        let mut actions = vec![vec![Action::command(
            if record.enabled { "Disable" } else { "Enable" },
            format!(
                "/skills {} {}",
                if record.enabled { "disable" } else { "enable" },
                record.id
            ),
        )]];
        if record.source_kind == "learned" {
            actions.push(vec![Action::command(
                "Delete",
                format!("/skills delete {}", record.id),
            )]);
        }
        actions.push(vec![Action::back(), Action::close()]);
        Ok(View {
            title: Some("SKILL DETAIL".into()),
            blocks: vec![
                Block::Table {
                    headers: vec!["Field".into(), "Value".into()],
                    rows: vec![
                        vec!["Name".into(), record.name],
                        vec!["Description".into(), record.summary],
                        vec!["Source".into(), record.source_kind],
                        vec![
                            "State".into(),
                            if record.enabled {
                                "ENABLED".into()
                            } else {
                                "DISABLED".into()
                            },
                        ],
                        vec!["Prerequisites".into(), prerequisites],
                        vec!["Updated".into(), record.updated_at],
                    ],
                },
                Block::Paragraph {
                    text: format!(
                        "When to use:\n{}\n\nProcedure:\n{}\n\nPitfalls:\n{}\n\nVerification:\n{}",
                        record.when_to_use, record.procedure, record.pitfalls, record.verification
                    ),
                },
            ],
            actions,
            side_mode: false,
        })
    }

    fn tools_view(&self) -> View {
        let capabilities = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.capabilities().list())
            .unwrap_or_default();
        let rows = capabilities
            .into_iter()
            .take(40)
            .map(|capability| {
                let marker = match capability.status {
                    CapabilityStatus::Available => "✓",
                    CapabilityStatus::MissingInstallable => "○",
                    CapabilityStatus::ApprovalRequired => "!",
                    _ => "×",
                };
                vec![
                    marker.into(),
                    capability.name,
                    capability.status.as_str().into(),
                ]
            })
            .collect();
        View {
            title: Some("TOOLS & CAPABILITIES".into()),
            blocks: vec![Block::Table {
                headers: vec!["".into(), "Capability".into(), "State".into()],
                rows,
            }],
            actions: vec![vec![Action::command("Refresh", "/tools"), Action::close()]],
            side_mode: false,
        }
    }

    fn about_view(&self) -> View {
        let environment = self.runtime.as_ref().map(|runtime| runtime.environment());
        let rows = environment
            .map(|environment| {
                vec![
                    vec!["Version".into(), crate::VERSION.into()],
                    vec!["Core".into(), "Rust".into()],
                    vec!["Interface".into(), "Telegram".into()],
                    vec!["Platform".into(), environment.platform],
                    vec!["Architecture".into(), environment.architecture],
                    vec![
                        "Root".into(),
                        if environment.root_available {
                            "available".into()
                        } else {
                            "unavailable".into()
                        },
                    ],
                    vec!["SELinux".into(), environment.selinux.as_str().into()],
                    vec![
                        "Termux".into(),
                        environment
                            .termux
                            .map(|termux| termux.prefix.display().to_string())
                            .unwrap_or_else(|| "unavailable".into()),
                    ],
                ]
            })
            .unwrap_or_else(|| {
                vec![
                    vec!["Version".into(), crate::VERSION.into()],
                    vec!["Runtime".into(), "unavailable".into()],
                ]
            });
        View {
            title: Some("XIAO\nPrivate Personal AI Agent".into()),
            blocks: vec![Block::Table {
                headers: vec!["Field".into(), "Value".into()],
                rows,
            }],
            actions: vec![vec![
                Action::command("Refresh", "/about"),
                Action::command("Tools", "/tools"),
                Action::close(),
            ]],
            side_mode: false,
        }
    }
}

fn normalize_provider(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "agy" | "antigravity" => "antigravity".into(),
        "codex" => "codex".into(),
        "custom" => "custom".into(),
        other => other.into(),
    }
}

fn login_picker() -> View {
    View {
        title: Some("LOGIN".into()),
        blocks: vec![Block::Paragraph { text: "Choose a provider. OAuth/API secrets are stored outside normal config and never echoed back in full.".into() }],
        actions: vec![
            vec![Action::command("Codex", "/login codex"), Action::command("AGY", "/login antigravity")],
            vec![Action::command("Custom", "/login custom"), Action::close()],
        ],
        side_mode: false,
    }
}

fn help_view() -> View {
    View {
        title: Some("HELP".into()),
        blocks: vec![Block::Paragraph {
            text: TelegramCommandRegistry::help_text(),
        }],
        actions: vec![vec![Action::close()]],
        side_mode: false,
    }
}

fn short_activity(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|d| d.format("%d %b %H:%M").to_string())
        .unwrap_or_else(|_| value.chars().take(16).collect())
}

pub fn account_label(account: &AccountRecord) -> String {
    account
        .email
        .clone()
        .unwrap_or_else(|| account.label.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{Provider, ProviderRequest, ProviderResponse};
    use async_trait::async_trait;

    struct FakeProvider {
        id: &'static str,
        models: Vec<String>,
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn id(&self) -> &'static str {
            self.id
        }
        fn models(&self) -> Vec<String> {
            self.models.clone()
        }
        fn ready(&self) -> bool {
            true
        }
        async fn run(
            &self,
            _: ProviderRequest,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            Ok(ProviderResponse {
                events: vec![],
                final_answer: "ok".into(),
            })
        }
    }

    fn account(id: &str, provider: &str) -> AccountRecord {
        AccountRecord {
            id: id.into(),
            provider: provider.into(),
            label: id.into(),
            email: Some(format!("{id}@example.test")),
            status: "connected".into(),
            access_expires_at: None,
            metadata_json: "{}".into(),
        }
    }

    fn core() -> (
        CommandCore,
        Arc<Storage>,
        Arc<SessionManager>,
        tempfile::TempDir,
    ) {
        let storage = Arc::new(Storage::open_memory().unwrap());
        let sessions = Arc::new(SessionManager::new(storage.clone()));
        let temp = tempfile::tempdir().unwrap();
        let auth = Arc::new(AuthManager::new(
            storage.clone(),
            temp.path().join("secrets"),
        ));
        let providers = Arc::new(ProviderRegistry::from_test(
            vec![
                (
                    "custom",
                    Arc::new(FakeProvider {
                        id: "custom",
                        models: (0..12)
                            .map(|index| format!("custom-model-{index:02}"))
                            .collect(),
                    }) as Arc<dyn Provider>,
                ),
                (
                    "codex",
                    Arc::new(FakeProvider {
                        id: "codex",
                        models: vec!["codex-default".into(), "codex-alt".into()],
                    }) as Arc<dyn Provider>,
                ),
                (
                    "antigravity",
                    Arc::new(FakeProvider {
                        id: "antigravity",
                        models: vec!["agy-default".into()],
                    }) as Arc<dyn Provider>,
                ),
                (
                    "empty",
                    Arc::new(FakeProvider {
                        id: "empty",
                        models: vec![],
                    }) as Arc<dyn Provider>,
                ),
            ],
            auth.clone(),
        ));
        let config = Arc::new(RwLock::new(AppConfig::default()));
        let health = Arc::new(HealthState::new());
        let events = Arc::new(EventBus::new(32));
        let core = CommandCore::new(
            config,
            storage.clone(),
            sessions.clone(),
            providers,
            auth,
            health,
            events,
        );
        (core, storage, sessions, temp)
    }

    #[test]
    fn use_account_atomically_activates_codex_from_fresh_custom_session() {
        let (core, storage, sessions, _temp) = core();
        let session = sessions.ensure_default_session("p").unwrap();
        storage.upsert_account(&account("c1", "codex")).unwrap();
        let (provider, model) = core.use_account("p", None, "c1").unwrap();
        assert_eq!(
            (provider.as_str(), model.as_str()),
            ("codex", "codex-default")
        );
        let after = storage.session("p", &session.id).unwrap().unwrap();
        assert_eq!(after.provider, "codex");
        assert_eq!(after.account_id.as_deref(), Some("c1"));
        assert_eq!(after.model, "codex-default");
    }

    #[tokio::test]
    async fn owner_can_inspect_approve_and_deny_pending_operations() {
        let (core, storage, _sessions, _temp) = core();
        let pending = storage
            .request_approval(
                "p",
                "android.service.restart",
                "android_xiao_restart",
                "hash-a",
                "restart Xiao service",
            )
            .unwrap();
        assert!(matches!(
            core.execute_text("p", "/approvals").await.unwrap(),
            CommandResult::ManagerView(_)
        ));
        assert!(matches!(
            core.execute_text("p", &format!("/approve {}", pending.id))
                .await
                .unwrap(),
            CommandResult::Confirmation(_)
        ));
        assert!(storage
            .consume_approval("p", "android_xiao_restart", "hash-a")
            .unwrap());
        assert!(!storage
            .consume_approval("p", "android_xiao_restart", "hash-a")
            .unwrap());

        let denied = storage
            .request_approval(
                "p",
                "android.service.restart",
                "android_xiao_restart",
                "hash-b",
                "restart Xiao service",
            )
            .unwrap();
        core.execute_text("p", &format!("/deny {}", denied.id))
            .await
            .unwrap();
        assert!(!storage
            .consume_approval("p", "android_xiao_restart", "hash-b")
            .unwrap());
        assert!(storage.pending_approvals("other-owner").unwrap().is_empty());
    }

    #[tokio::test]
    async fn memory_and_skill_owner_managers_edit_disable_and_guard_delete() {
        let (core, storage, _sessions, _temp) = core();
        let memory = MemoryStore::new(storage.clone());
        memory
            .upsert(
                "p",
                MemoryScope::User,
                "preference",
                "diagram_style",
                "PlantUML",
                1.0,
                "test",
                None,
            )
            .unwrap();
        assert!(matches!(
            core.execute_text("p", "/memory user").await.unwrap(),
            CommandResult::ManagerView(_)
        ));
        core.execute_text("p", "/memory edit user preference diagram_style Mermaid")
            .await
            .unwrap();
        assert_eq!(
            memory.list("p", Some(MemoryScope::User), 10).unwrap()[0].value,
            "Mermaid"
        );

        let skill_store = SkillStore::new(storage);
        let skill = skill_store
            .create_or_update(
                "p",
                crate::skills::SkillCandidate {
                    name: "verify-owner-workflow".into(),
                    summary: "Verify an owner workflow".into(),
                    when_to_use: "When the owner asks to verify a workflow".into(),
                    prerequisites: "A read-only status capability".into(),
                    procedure: "1. Inspect status.\n2. Verify the outcome.".into(),
                    pitfalls: "Do not infer success from prose.".into(),
                    verification: "A successful status observation.".into(),
                },
                Some("session"),
            )
            .unwrap()
            .1;
        assert!(matches!(
            core.execute_text("p", "/skills learned").await.unwrap(),
            CommandResult::ManagerView(_)
        ));
        let detail = core
            .execute_text("p", &format!("/skills detail {}", skill.id))
            .await
            .unwrap();
        let CommandResult::ManagerView(detail) = detail else {
            panic!("skill detail manager expected")
        };
        assert!(serde_json::to_string(&detail)
            .unwrap()
            .contains("Prerequisites"));
        core.execute_text("p", &format!("/skills disable {}", skill.id))
            .await
            .unwrap();
        assert!(!skill_store.view("p", &skill.id).unwrap().unwrap().enabled);
        assert!(matches!(
            core.execute_text("p", &format!("/skills delete {}", skill.id))
                .await
                .unwrap(),
            CommandResult::ManagerView(_)
        ));
        core.execute_text("p", &format!("/skills delete-confirm {}", skill.id))
            .await
            .unwrap();
        assert!(skill_store.view("p", &skill.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn topic_session_manager_paginates_and_preserves_archived_history() {
        let (core, storage, sessions, _temp) = core();
        let owner = "telegram:100:10";
        let scope = TelegramScope::new(100, Some(44));
        let initial = sessions.context_for_telegram(owner, scope).unwrap().main;
        for _ in 0..6 {
            assert!(matches!(
                core.execute_text_in_telegram_scope(owner, scope, "/new", None)
                    .await
                    .unwrap(),
                CommandResult::Confirmation(_)
            ));
        }

        let first_page = core
            .execute_text_in_telegram_scope(owner, scope, "/sessions", None)
            .await
            .unwrap();
        let CommandResult::ManagerView(first_page) = first_page else {
            panic!("session manager view expected")
        };
        let first_rows = first_page
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { rows, .. } => Some(rows),
                _ => None,
            })
            .unwrap();
        assert_eq!(first_rows.len(), 5);
        assert!(first_page
            .actions
            .iter()
            .flatten()
            .any(|action| action.label == "Close"));

        let second_page = core
            .execute_text_in_telegram_scope(owner, scope, "/session 2", None)
            .await
            .unwrap();
        let CommandResult::ManagerView(second_page) = second_page else {
            panic!("hidden /session alias must open the manager")
        };
        let second_rows = second_page
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { rows, .. } => Some(rows),
                _ => None,
            })
            .unwrap();
        assert_eq!(second_rows.len(), 2);

        let active = sessions.context_for_telegram(owner, scope).unwrap().main;
        storage
            .append_message(owner, &active.id, "user", "archived recall sentinel")
            .unwrap();
        core.execute_text_in_telegram_scope(
            owner,
            scope,
            &format!("/sessions rename {} Project Atlas", active.id),
            None,
        )
        .await
        .unwrap();
        let renamed = storage.session(owner, &active.id).unwrap().unwrap();
        assert_eq!(renamed.name, "Project Atlas");
        assert!(matches!(
            core.execute_text_in_telegram_scope(
                owner,
                scope,
                &format!("/sessions detail {}", active.id),
                None,
            )
            .await
            .unwrap(),
            CommandResult::ManagerView(_)
        ));
        core.execute_text_in_telegram_scope(
            owner,
            scope,
            &format!("/sessions archive {}", active.id),
            None,
        )
        .await
        .unwrap();
        assert!(
            storage
                .session(owner, &active.id)
                .unwrap()
                .unwrap()
                .archived
        );
        assert_eq!(
            storage.messages(owner, &active.id).unwrap()[0].content,
            "archived recall sentinel"
        );
        assert_ne!(
            sessions.context_for_telegram(owner, scope).unwrap().main.id,
            active.id
        );
        assert!(storage.session(owner, &initial.id).unwrap().is_some());
    }

    #[tokio::test]
    async fn model_picker_and_runtime_command_surfaces_are_bounded_and_factual() {
        let (core, _storage, _sessions, _temp) = core();
        let first = core.execute_text("p", "/model change 1").await.unwrap();
        let CommandResult::ManagerView(first) = first else {
            panic!("model picker expected")
        };
        let rows = first
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { rows, .. } => Some(rows),
                _ => None,
            })
            .unwrap();
        assert_eq!(rows.len(), 5);
        let second = core.execute_text("p", "/model change 2").await.unwrap();
        let CommandResult::ManagerView(second) = second else {
            panic!("second model page expected")
        };
        let second_rows = second
            .blocks
            .iter()
            .find_map(|block| match block {
                Block::Table { rows, .. } => Some(rows),
                _ => None,
            })
            .unwrap();
        assert_eq!(second_rows.len(), 5);

        for (command, expected) in [
            ("/status", "YOLO"),
            ("/context", "Context budget"),
            ("/tools", "TOOLS & CAPABILITIES"),
            ("/doctor", "Root AI shell: DISABLED"),
            ("/about", "Private Personal AI Agent"),
            ("/approvals", "No pending"),
        ] {
            let result = core.execute_text("p", command).await.unwrap();
            let rendered = serde_json::to_string(&match result {
                CommandResult::InfoView(view) | CommandResult::ManagerView(view) => view,
                other => panic!("unexpected result for {command}: {other:?}"),
            })
            .unwrap();
            assert!(rendered.contains(expected), "{command} missing {expected}");
        }
    }

    #[test]
    fn use_account_atomically_activates_agy_from_fresh_custom_session() {
        let (core, storage, sessions, _temp) = core();
        let session = sessions.ensure_default_session("p").unwrap();
        storage
            .upsert_account(&account("a1", "antigravity"))
            .unwrap();
        core.use_account("p", None, "a1").unwrap();
        let after = storage.session("p", &session.id).unwrap().unwrap();
        assert_eq!(after.provider, "antigravity");
        assert_eq!(after.account_id.as_deref(), Some("a1"));
        assert_eq!(after.model, "agy-default");
    }

    #[test]
    fn use_account_switches_between_same_and_different_providers() {
        let (core, storage, sessions, _temp) = core();
        let session = sessions.ensure_default_session("p").unwrap();
        for a in [
            account("c1", "codex"),
            account("c2", "codex"),
            account("a1", "antigravity"),
        ] {
            storage.upsert_account(&a).unwrap();
        }
        core.use_account("p", None, "c1").unwrap();
        core.use_account("p", None, "c2").unwrap();
        let codex = storage.session("p", &session.id).unwrap().unwrap();
        assert_eq!(codex.account_id.as_deref(), Some("c2"));
        assert_eq!(codex.provider, "codex");
        core.use_account("p", None, "a1").unwrap();
        let agy = storage.session("p", &session.id).unwrap().unwrap();
        assert_eq!(agy.account_id.as_deref(), Some("a1"));
        assert_eq!(agy.provider, "antigravity");
        assert_eq!(agy.model, "agy-default");
    }

    #[test]
    fn use_account_invalid_or_deleted_account_leaves_session_unchanged() {
        let (core, storage, sessions, _temp) = core();
        let session = sessions.ensure_default_session("p").unwrap();
        let before = storage.session("p", &session.id).unwrap().unwrap();
        assert!(core.use_account("p", None, "missing").is_err());
        let after = storage.session("p", &session.id).unwrap().unwrap();
        assert_eq!(
            (after.provider, after.account_id, after.model),
            (before.provider, before.account_id, before.model)
        );
        storage.upsert_account(&account("gone", "codex")).unwrap();
        storage.delete_account("gone").unwrap();
        assert!(core.use_account("p", None, "gone").is_err());
    }

    #[test]
    fn use_account_no_models_rolls_back_all_session_fields() {
        let (core, storage, sessions, _temp) = core();
        let session = sessions.ensure_default_session("p").unwrap();
        storage.upsert_account(&account("e1", "empty")).unwrap();
        let before = storage.session("p", &session.id).unwrap().unwrap();
        assert!(core
            .use_account("p", None, "e1")
            .unwrap_err()
            .to_string()
            .contains("no usable models"));
        let after = storage.session("p", &session.id).unwrap().unwrap();
        assert_eq!(
            (after.provider, after.account_id, after.model),
            (before.provider, before.account_id, before.model)
        );
    }

    #[test]
    fn storage_transaction_rejects_disconnected_account_without_partial_switch() {
        let (core, storage, sessions, _temp) = core();
        let session = sessions.ensure_default_session("p").unwrap();
        let mut disconnected = account("c1", "codex");
        disconnected.status = "expired".into();
        storage.upsert_account(&disconnected).unwrap();
        let before = storage.session("p", &session.id).unwrap().unwrap();
        assert!(core.use_account("p", None, "c1").is_err());
        let after = storage.session("p", &session.id).unwrap().unwrap();
        assert_eq!(
            (after.provider, after.account_id, after.model),
            (before.provider, before.account_id, before.model)
        );
    }
}
