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
    presentation::{Action, Block, View},
    providers::{AgentEvent, ProviderRegistry, ProviderState},
    runtime::RuntimeState,
    session::{ChatMode, SessionManager},
    storage::{AccountRecord, Storage},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help { topic: Option<String> },
    Login { provider: Option<String> },
    CancelAuth { transaction: String },
    Logout { account: Option<String> },
    Provider,
    SetProvider { provider: String },
    Account,
    UseAccount { account: String },
    Model,
    ModelPicker,
    SetModel { model: String },
    NewSession,
    Session { page: usize },
    SessionDetail { session: String },
    SwitchSession { session: String },
    RequestRenameSession { session: String },
    RenameSession { session: String, name: String },
    ArchiveSession { session: String },
    ToggleSideChat,
    Status,
    Context,
    Stop,
    Retry,
    Approvals,
    Approve { request: String },
    DenyApproval { request: String },
    Settings,
    SetProgressDetail { detail: String },
    SetMenuCloseBehavior { behavior: String },
    Usage,
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

    Ok(Some(match name.as_str() {
        "new" => Command::NewSession,
        "btw" => Command::ToggleSideChat,
        "session" if args.first() == Some(&"detail") => Command::SessionDetail {
            session: args
                .get(1)
                .ok_or_else(|| anyhow!("session id required"))?
                .to_string(),
        },
        "session" if args.first() == Some(&"switch") => Command::SwitchSession {
            session: args
                .get(1)
                .ok_or_else(|| anyhow!("session id required"))?
                .to_string(),
        },
        "session" if args.first() == Some(&"archive") => Command::ArchiveSession {
            session: args
                .get(1)
                .ok_or_else(|| anyhow!("session id required"))?
                .to_string(),
        },
        "session" if args.first() == Some(&"rename") && args.len() == 2 => {
            Command::RequestRenameSession {
                session: args[1].to_string(),
            }
        }
        "session" if args.first() == Some(&"rename") => Command::RenameSession {
            session: args
                .get(1)
                .ok_or_else(|| anyhow!("session id required"))?
                .to_string(),
            name: args.iter().skip(2).copied().collect::<Vec<_>>().join(" "),
        },
        "session" => Command::Session {
            page: one().and_then(|s| s.parse().ok()).unwrap_or(1),
        },
        "model" if args.first() == Some(&"change") => Command::ModelPicker,
        "model" if !args.is_empty() => Command::SetModel {
            model: args.join(" "),
        },
        "model" => Command::Model,
        "provider" if !args.is_empty() => Command::SetProvider {
            provider: args[0].to_owned(),
        },
        "provider" => Command::Provider,
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
        "stop" => Command::Stop,
        "retry" => Command::Retry,
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
        "settings" if args.first() == Some(&"progress") => Command::SetProgressDetail {
            detail: args
                .get(1)
                .ok_or_else(|| anyhow!("progress detail required"))?
                .to_string(),
        },
        "settings" if args.first() == Some(&"close") => Command::SetMenuCloseBehavior {
            behavior: args
                .get(1)
                .ok_or_else(|| anyhow!("close behavior required"))?
                .to_string(),
        },
        "settings" => Command::Settings,
        "usage" => Command::Usage,
        "doctor" => Command::Doctor,
        "help" | "start" => Command::Help { topic: one() },
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
        let agent = if let Some(runtime) = runtime {
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
        match parse(input)? {
            Some(cmd) => self.execute(principal, cmd).await,
            None => {
                if !self.config.read().await.gateway.enabled {
                    return Err(anyhow!("gateway is disabled"));
                }
                Ok(CommandResult::Agent(
                    self.agent
                        .submit_with_progress(principal, input, progress)
                        .await?,
                ))
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

    pub async fn execute(&self, principal: &str, command: Command) -> Result<CommandResult> {
        use Command::*;
        match command {
            NewSession => {
                let s = self.sessions.create_and_switch(principal)?;
                self.events.publish(AppEvent::SessionChanged {
                    principal: principal.into(),
                    session_id: s.id.clone(),
                });
                let mut view =
                    View::info("NEW SESSION", format!("Created and activated {}", s.name));
                view.actions = vec![vec![
                    Action::command("Session Manager", "/session"),
                    Action::close(),
                ]];
                Ok(CommandResult::Confirmation(view))
            }
            ToggleSideChat => {
                let c = self.sessions.toggle_side(principal)?;
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
                self.session_view(principal, page)?,
            )),
            SessionDetail { session } => Ok(CommandResult::ManagerView(
                self.session_detail_view(principal, &session)?,
            )),
            SwitchSession { session } => {
                let s = self.sessions.switch_main(principal, &session)?;
                self.events.publish(AppEvent::SessionChanged {
                    principal: principal.into(),
                    session_id: s.id.clone(),
                });
                Ok(CommandResult::ManagerView(self.session_view_with_notice(
                    principal,
                    1,
                    format!("Active: {}", s.name),
                )?))
            }
            RequestRenameSession { session } => {
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
                    command_prefix: format!("/session rename {}", target.id),
                })
            }
            RenameSession { session, name } => {
                let name = name.trim();
                if name.is_empty() {
                    return Err(anyhow!("new session name required"));
                }
                if name.chars().count() > 120 {
                    return Err(anyhow!("session name must be 120 characters or fewer"));
                }
                self.storage.rename_session(principal, &session, name)?;
                Ok(CommandResult::ManagerView(
                    self.session_detail_view(principal, &session)?,
                ))
            }
            ArchiveSession { session } => {
                let active = self.sessions.archive_and_recover(principal, &session)?;
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
                    1,
                    format!("Archived. Active: {}", active.name),
                )?))
            }
            Provider => Ok(CommandResult::ManagerView(self.provider_view(principal)?)),
            SetProvider { provider } => {
                self.set_provider(principal, &provider)?;
                self.events.publish(AppEvent::ProviderChanged {
                    principal: principal.into(),
                    provider: normalize_provider(&provider),
                });
                Ok(CommandResult::ManagerView(self.provider_view(principal)?))
            }
            Model => Ok(CommandResult::ManagerView(self.model_view(principal)?)),
            ModelPicker => Ok(CommandResult::ManagerView(
                self.model_picker_view(principal)?,
            )),
            SetModel { model } => {
                self.set_model(principal, &model)?;
                self.events.publish(AppEvent::ModelChanged {
                    principal: principal.into(),
                    model,
                });
                Ok(CommandResult::ManagerView(self.model_view(principal)?))
            }
            Account => Ok(CommandResult::ManagerView(self.account_view(principal)?)),
            UseAccount { account } => {
                let (provider, model) = self.use_account(principal, &account)?;
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
                Ok(CommandResult::ManagerView(self.model_view(principal)?))
            }
            Login { provider: None } => Ok(CommandResult::ManagerView(login_picker())),
            Login {
                provider: Some(provider),
            } => {
                let provider = normalize_provider(&provider);
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
            Status => Ok(CommandResult::InfoView(self.status_view(principal).await?)),
            Context => Ok(CommandResult::InfoView(self.context_view(principal)?)),
            Stop => Ok(CommandResult::Confirmation(View::info(
                "STOP",
                if self.agent.cancel(principal) {
                    "Cancellation requested"
                } else {
                    "No active generation"
                },
            ))),
            Retry => Ok(CommandResult::Agent(
                self.agent.retry_with_progress(principal, None).await?,
            )),
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
            Settings => Ok(CommandResult::ManagerView(
                self.settings_view(principal).await?,
            )),
            SetProgressDetail { detail } => {
                if !matches!(detail.as_str(), "minimal" | "normal" | "detailed") {
                    return Err(anyhow!(
                        "progress detail must be minimal, normal, or detailed"
                    ));
                }
                self.storage
                    .put_setting(&format!("telegram.progress_detail:{principal}"), &detail)?;
                Ok(CommandResult::ManagerView(
                    self.settings_view(principal).await?,
                ))
            }
            SetMenuCloseBehavior { behavior } => {
                if !matches!(
                    behavior.as_str(),
                    "keep_summary" | "remove_keyboard" | "delete_message"
                ) {
                    return Err(anyhow!(
                        "close behavior must be keep_summary, remove_keyboard, or delete_message"
                    ));
                }
                self.storage.put_setting(
                    &format!("telegram.menu_close_behavior:{principal}"),
                    &behavior,
                )?;
                Ok(CommandResult::ManagerView(
                    self.settings_view(principal).await?,
                ))
            }
            Help { topic } => Ok(CommandResult::ManagerView(help_view(topic.as_deref()))),
            Usage => Ok(CommandResult::InfoView(self.usage_view(principal)?)),
            Doctor => Ok(CommandResult::InfoView(self.doctor_view().await)),
        }
    }

    fn session_view(&self, principal: &str, page: usize) -> Result<View> {
        self.session_view_with_notice(principal, page, String::new())
    }

    fn session_view_with_notice(
        &self,
        principal: &str,
        page: usize,
        notice: String,
    ) -> Result<View> {
        let (rows, pages, page) = self.sessions.list_page(principal, page, 5)?;
        let active = self.sessions.context_for(principal)?.main.id;
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
                        Action::command((i + 1).to_string(), format!("/session switch {}", s.id))
                    })
                    .collect(),
            );
        }
        actions.push(vec![
            Action::command("‹", format!("/session {}", page.saturating_sub(1).max(1))),
            Action::noop(format!("{page}/{pages}")),
            Action::command("›", format!("/session {}", (page + 1).min(pages))),
        ]);
        actions.push(vec![
            Action::command("New", "/new"),
            Action::command("Rename", format!("/session rename {active}")),
            Action::command("Detail", format!("/session detail {active}")),
        ]);
        actions.push(vec![
            Action::command("Archive", format!("/session archive {active}")),
            Action::close(),
        ]);
        Ok(View {
            title: Some("SESSION".into()),
            blocks,
            actions,
            side_mode: false,
        })
    }

    fn session_detail_view(&self, principal: &str, id: &str) -> Result<View> {
        let s = self
            .storage
            .session(principal, id)?
            .ok_or_else(|| anyhow!("session not found"))?;
        if s.is_side {
            return Err(anyhow!(
                "side sessions are not exposed in the main session manager"
            ));
        }
        let active = self.sessions.context_for(principal)?.main.id == s.id;
        let mode = if active {
            self.sessions.context_for(principal)?.mode.as_str()
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
            Action::command("Select", format!("/session switch {}", s.id)),
            Action::command("Rename", format!("/session rename {}", s.id)),
        ]];
        actions.push(vec![
            Action::command("Archive", format!("/session archive {}", s.id)),
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

    fn provider_view(&self, principal: &str) -> Result<View> {
        let current = self.sessions.context_for(principal)?.active.provider;
        let rows = self
            .providers
            .list()
            .into_iter()
            .map(|p| {
                vec![
                    if p == current {
                        "●".into()
                    } else {
                        "".into()
                    },
                    p,
                ]
            })
            .collect();
        let actions = self
            .providers
            .list()
            .into_iter()
            .map(|p| vec![Action::command(p.clone(), format!("/provider {p}"))])
            .collect();
        Ok(View {
            title: Some("PROVIDER".into()),
            blocks: vec![Block::Table {
                headers: vec!["Current".into(), "Provider".into()],
                rows,
            }],
            actions,
            side_mode: false,
        })
    }

    fn model_view(&self, principal: &str) -> Result<View> {
        let c = self.sessions.context_for(principal)?;
        let account = c
            .active
            .account_id
            .as_deref()
            .and_then(|id| self.storage.account(id).ok().flatten())
            .map(|a| account_label(&a))
            .unwrap_or_else(|| "—".into());
        let status = format!("{:?}", self.providers.state(&c.active.provider)).to_uppercase();
        Ok(View {
            title: Some("MODEL".into()),
            blocks: vec![Block::Table {
                headers: vec!["Field".into(), "Value".into()],
                rows: vec![
                    vec!["PROVIDER".into(), c.active.provider.clone()],
                    vec!["ACCOUNT".into(), account],
                    vec!["MODEL".into(), c.active.model.clone()],
                    vec!["REASONING".into(), "Provider default".into()],
                    vec!["SESSION".into(), c.main.name],
                    vec!["STATUS".into(), status],
                ],
            }],
            actions: vec![
                vec![Action::command("Change Model", "/model change")],
                vec![
                    Action::command("Account", "/account"),
                    Action::command("Provider", "/provider"),
                ],
                vec![Action::close()],
            ],
            side_mode: c.mode == ChatMode::Side,
        })
    }

    fn model_picker_view(&self, principal: &str) -> Result<View> {
        let c = self.sessions.context_for(principal)?;
        let models = self.providers.models(&c.active.provider)?;
        let rows = models
            .iter()
            .map(|m| {
                vec![
                    if *m == c.active.model {
                        "●".into()
                    } else {
                        "".into()
                    },
                    m.clone(),
                ]
            })
            .collect();
        let mut actions = models
            .into_iter()
            .map(|m| vec![Action::command(m.clone(), format!("/model {m}"))])
            .collect::<Vec<_>>();
        actions.push(vec![Action::back(), Action::close()]);
        Ok(View {
            title: Some("MODEL".into()),
            blocks: vec![Block::Table {
                headers: vec!["Current".into(), "Model".into()],
                rows,
            }],
            actions,
            side_mode: false,
        })
    }

    fn account_view(&self, principal: &str) -> Result<View> {
        let c = self.sessions.context_for(principal)?;
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

    fn set_provider(&self, principal: &str, provider: &str) -> Result<()> {
        let provider = normalize_provider(provider);
        self.providers.get(&provider)?;
        let c = self.sessions.context_for(principal)?;
        let account = self
            .auth
            .accounts(Some(&provider))?
            .into_iter()
            .find(|a| a.status == "connected")
            .map(|a| a.id);
        let default = self.providers.preferred_model(&provider)?;
        self.storage.set_session_provider(
            principal,
            &c.active.id,
            &provider,
            account.as_deref(),
            &default,
        )
    }

    fn set_model(&self, principal: &str, model: &str) -> Result<()> {
        let c = self.sessions.context_for(principal)?;
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

    /// Atomic provider/account/model activation. Provider and model are resolved before
    /// the transaction; the storage layer then updates all three fields together.
    fn use_account(&self, principal: &str, account: &str) -> Result<(String, String)> {
        let record = self
            .storage
            .account(account)?
            .ok_or_else(|| anyhow!("account not found"))?;
        if record.status != "connected" {
            return Err(anyhow!("account is not connected"));
        }
        let model = self.providers.preferred_model(&record.provider)?;
        let c = self.sessions.context_for(principal)?;
        self.storage.activate_account(
            principal,
            &c.active.id,
            account,
            &record.provider,
            &model,
        )?;
        Ok((record.provider, model))
    }

    async fn status_view(&self, principal: &str) -> Result<View> {
        let cfg = self.config.read().await.clone();
        let health = self
            .health
            .snapshot(&cfg, self.storage.health(), self.providers.states())
            .await;
        let c = self.sessions.context_for(principal)?;
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
        Ok(View {
            title: Some("STATUS".into()),
            blocks: vec![Block::Table {
                headers: vec!["Field".into(), "Value".into()],
                rows: vec![
                    vec!["Gateway".into(), format!("{:?}", gateway).to_uppercase()],
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
                    vec!["Mode".into(), c.mode.as_str().to_uppercase()],
                ],
            }],
            actions: vec![],
            side_mode: c.mode == ChatMode::Side,
        })
    }

    fn context_view(&self, principal: &str) -> Result<View> {
        let c = self.sessions.context_for(principal)?;
        let effective = self.sessions.agent_context(principal)?;
        Ok(View {
            title: Some("CONTEXT".into()),
            blocks: vec![Block::Table {
                headers: vec!["Field".into(), "Value".into()],
                rows: vec![
                    vec!["Main messages".into(), c.main.message_count.to_string()],
                    vec!["Effective messages".into(), effective.len().to_string()],
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

    fn usage_view(&self, principal: &str) -> Result<View> {
        let c = self.sessions.context_for(principal)?;
        let stored = self.storage.messages(principal, &c.active.id)?;
        let chars: usize = stored.iter().map(|m| m.content.chars().count()).sum();
        Ok(View::info("USAGE", format!("Session messages: {}\nStored characters: {}\nProvider quota telemetry is provider-dependent and not normalized in v0.2.0.", stored.len(), chars)))
    }

    async fn doctor_view(&self) -> View {
        let cfg = self.config.read().await.clone();
        let checks = vec![
            format!("DB: {}", if self.storage.health() { "OK" } else { "ERROR" }),
            format!("IPC: {}", cfg.ipc.bind),
            format!("Telegram transport: {}", cfg.telegram.transport),
            format!("Providers registered: {}", self.providers.readiness()),
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

    async fn settings_view(&self, principal: &str) -> Result<View> {
        let cfg = self.config.read().await;
        let detail = self
            .storage
            .setting(&format!("telegram.progress_detail:{principal}"))?
            .unwrap_or_else(|| cfg.telegram.ui.progress_detail.clone());
        let close = self
            .storage
            .setting(&format!("telegram.menu_close_behavior:{principal}"))?
            .unwrap_or_else(|| cfg.telegram.ui.menu_close_behavior.clone());
        let mark = |x: &str| if x == detail { "●" } else { "○" };
        Ok(View {
            title: Some("SETTINGS".into()),
            blocks: vec![Block::Table {
                headers: vec!["Setting".into(), "Value".into()],
                rows: vec![
                    vec!["Agent Progress".into(), detail.clone()],
                    vec!["Menu Close".into(), close.clone()],
                ],
            }],
            actions: vec![
                vec![
                    Action::command(
                        format!("{} Minimal", mark("minimal")),
                        "/settings progress minimal",
                    ),
                    Action::command(
                        format!("{} Normal", mark("normal")),
                        "/settings progress normal",
                    ),
                    Action::command(
                        format!("{} Detailed", mark("detailed")),
                        "/settings progress detailed",
                    ),
                ],
                vec![
                    Action::command("Keep summary", "/settings close keep_summary"),
                    Action::command("Remove keyboard", "/settings close remove_keyboard"),
                ],
                vec![
                    Action::command("Delete message", "/settings close delete_message"),
                    Action::close(),
                ],
            ],
            side_mode: false,
        })
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

fn help_view(topic: Option<&str>) -> View {
    let topic = topic.unwrap_or("root").to_ascii_lowercase();
    let (title,text)=match topic.as_str(){
        "chat"|"session"|"btw"=>("HELP · CHAT","/new — create a main session\n/btw — enter/exit isolated side chat\n/session — manage only your sessions\n/retry — repeat your latest user request\n/stop — cancel active generation"),
        "ai"|"model"|"provider"=>("HELP · AI","/model — active provider/account/model status\n/provider — choose provider\n/context — effective context summary"),
        "accounts"|"account"|"login"=>("HELP · ACCOUNTS","/login [codex|antigravity] — authenticate\n/account — connected accounts\n/account <id> — atomically activate provider/account/model\n/logout [id] — disconnect"),
        "advanced"|"settings"=>("HELP · ADVANCED","/status — aggregate gateway health\n/approvals — inspect pending sensitive operations\n/approve <id> or /deny <id> — decide one exact operation\n/settings — Telegram progress/close behavior\n/usage — local session usage\n/doctor — diagnostic summary"),
        _=>("HELP","Choose a category or use /help btw, /help session, /help model, /help account, or /help settings."),
    };
    View {
        title: Some(title.into()),
        blocks: vec![Block::Paragraph { text: text.into() }],
        actions: vec![
            vec![
                Action::command("Chat", "/help chat"),
                Action::command("AI", "/help ai"),
            ],
            vec![
                Action::command("Accounts", "/help accounts"),
                Action::command("Advanced", "/help advanced"),
            ],
            vec![Action::close()],
        ],
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
                        models: vec!["custom-default".into()],
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
        let (provider, model) = core.use_account("p", "c1").unwrap();
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

    #[test]
    fn use_account_atomically_activates_agy_from_fresh_custom_session() {
        let (core, storage, sessions, _temp) = core();
        let session = sessions.ensure_default_session("p").unwrap();
        storage
            .upsert_account(&account("a1", "antigravity"))
            .unwrap();
        core.use_account("p", "a1").unwrap();
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
        core.use_account("p", "c1").unwrap();
        core.use_account("p", "c2").unwrap();
        let codex = storage.session("p", &session.id).unwrap().unwrap();
        assert_eq!(codex.account_id.as_deref(), Some("c2"));
        assert_eq!(codex.provider, "codex");
        core.use_account("p", "a1").unwrap();
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
        assert!(core.use_account("p", "missing").is_err());
        let after = storage.session("p", &session.id).unwrap().unwrap();
        assert_eq!(
            (after.provider, after.account_id, after.model),
            (before.provider, before.account_id, before.model)
        );
        storage.upsert_account(&account("gone", "codex")).unwrap();
        storage.delete_account("gone").unwrap();
        assert!(core.use_account("p", "gone").is_err());
    }

    #[test]
    fn use_account_no_models_rolls_back_all_session_fields() {
        let (core, storage, sessions, _temp) = core();
        let session = sessions.ensure_default_session("p").unwrap();
        storage.upsert_account(&account("e1", "empty")).unwrap();
        let before = storage.session("p", &session.id).unwrap().unwrap();
        assert!(core
            .use_account("p", "e1")
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
        assert!(core.use_account("p", "c1").is_err());
        let after = storage.session("p", &session.id).unwrap().unwrap();
        assert_eq!(
            (after.provider, after.account_id, after.model),
            (before.provider, before.account_id, before.model)
        );
    }
}
