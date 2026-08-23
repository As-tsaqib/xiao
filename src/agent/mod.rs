use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

mod completion;

pub use completion::{CompletionEvidence, CompletionVerifier, TaskKind, VerificationState};

use anyhow::{anyhow, Result};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    config::AgentConfig,
    context::{ContextEngine, SessionHistoryStore},
    learning::{LearningEvaluator, LearningTrace, SafeToolObservation},
    memory::{MemoryEvaluator, MemoryStore},
    providers::{AgentEvent, ProviderRegistry, ProviderRequest, ProviderStep},
    runtime::{
        DependencyResolver, ProcessExecutor, RuntimeState, SystemAndroidBroker, TermuxExecutor,
        TermuxPackageBackend,
    },
    security::redact::{redact_json, redact_text},
    session::{ChatMode, SessionManager},
    skills::{FilesystemSkills, SkillRegistry, SkillStore},
    storage::Storage,
    tools::{
        builtin::{
            AndroidXiaoRestartTool, AndroidXiaoStatusTool, ContextStatsTool, MemoryDeleteTool,
            MemorySearchTool, MemorySetTool, SkillSearchTool, SkillViewTool, TermuxTerminalTool,
        },
        ToolContext, ToolPolicy, ToolRegistry, ToolResult,
    },
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentAnswer {
    /// Safe status events only. No private reasoning or provider hidden chain-of-thought is represented here.
    pub progress: Vec<AgentEvent>,
    /// Persistent user-facing final answer only.
    pub final_answer: String,
    pub side_mode: bool,
    #[serde(default)]
    pub artifacts: Vec<AgentArtifact>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AgentArtifact {
    pub path: std::path::PathBuf,
    pub name: String,
    pub size_bytes: u64,
}

struct LoopOutcome {
    final_answer: String,
    verification: CompletionEvidence,
}

pub struct AgentEngine {
    sessions: Arc<SessionManager>,
    storage: Arc<Storage>,
    providers: Arc<ProviderRegistry>,
    active: Mutex<HashMap<String, CancellationToken>>,
    tools: Arc<ToolRegistry>,
    config: AgentConfig,
    memory_evaluator: MemoryEvaluator,
    context_engine: ContextEngine,
    completion: CompletionVerifier,
    learning: LearningEvaluator,
}

impl AgentEngine {
    pub fn new(
        sessions: Arc<SessionManager>,
        storage: Arc<Storage>,
        providers: Arc<ProviderRegistry>,
    ) -> Self {
        Self::with_config(sessions, storage, providers, AgentConfig::default())
    }

    pub fn with_config(
        sessions: Arc<SessionManager>,
        storage: Arc<Storage>,
        providers: Arc<ProviderRegistry>,
        config: AgentConfig,
    ) -> Self {
        let tools = Arc::new(ToolRegistry::new(
            ToolPolicy::default(),
            config.tool_output_max_chars,
        ));
        // A static built-in with a validated canonical spec cannot fail to
        // register in a fresh registry.
        tools
            .register(ContextStatsTool)
            .expect("register context_stats tool");
        let memory = Arc::new(MemoryStore::new(storage.clone()));
        tools
            .register(MemorySearchTool::new(memory.clone()))
            .expect("register memory_search tool");
        tools
            .register(MemorySetTool::new(memory.clone()))
            .expect("register memory_set tool");
        tools
            .register(MemoryDeleteTool::new(memory))
            .expect("register memory_delete tool");
        tools
            .register(crate::tools::builtin::SessionSearchTool::new(Arc::new(
                SessionHistoryStore::new(storage.clone()),
            )))
            .expect("register session_search tool");
        let skills = Arc::new(SkillRegistry::new(Arc::new(SkillStore::new(
            storage.clone(),
        ))));
        tools
            .register(SkillSearchTool::new(skills.clone()))
            .expect("register skill_search tool");
        tools
            .register(SkillViewTool::new(skills))
            .expect("register skill_view tool");
        Self::with_registry(sessions, storage, providers, config, tools)
    }

    pub fn with_runtime(
        sessions: Arc<SessionManager>,
        storage: Arc<Storage>,
        providers: Arc<ProviderRegistry>,
        config: AgentConfig,
        runtime: Arc<RuntimeState>,
    ) -> Self {
        let tools = Arc::new(ToolRegistry::with_runtime(
            ToolPolicy::default(),
            config.tool_output_max_chars,
            runtime.capabilities(),
            storage.clone(),
        ));
        tools
            .register(ContextStatsTool)
            .expect("register context_stats tool");
        let memory = Arc::new(MemoryStore::with_workspace(
            storage.clone(),
            runtime.workspace(),
        ));
        tools
            .register(MemorySearchTool::new(memory.clone()))
            .expect("register memory_search tool");
        tools
            .register(MemorySetTool::new(memory.clone()))
            .expect("register memory_set tool");
        tools
            .register(MemoryDeleteTool::new(memory))
            .expect("register memory_delete tool");
        tools
            .register(crate::tools::builtin::SessionSearchTool::new(Arc::new(
                SessionHistoryStore::new(storage.clone()),
            )))
            .expect("register session_search tool");
        let environment = runtime.environment();
        let mut skill_dependency_resolver = None;
        if let Some(termux) = environment.termux.clone() {
            // The identity workspace is normally root-owned under /data/adb.
            // General commands are deliberately dropped to the Termux app UID,
            // so their default/package-manager cwd must be the Termux home.
            let termux_home = termux.home.clone();
            let executor: Arc<dyn ProcessExecutor> = Arc::new(TermuxExecutor::new(
                termux.clone(),
                runtime.workspace().root(),
            ));
            let package_backend = Arc::new(TermuxPackageBackend::new(
                executor.clone(),
                termux,
                termux_home.clone(),
            ));
            let resolver = Arc::new(DependencyResolver::new(
                runtime.capabilities(),
                package_backend,
                Some(storage.clone()),
            ));
            skill_dependency_resolver = Some(resolver.clone());
            tools
                .register(TermuxTerminalTool::new(executor, resolver, termux_home))
                .expect("register Termux terminal tool");
            tools
                .register_alias("terminal", "termux_terminal")
                .expect("register terminal compatibility alias");
            tools
                .register_alias("exec", "termux_terminal")
                .expect("register exec compatibility alias");
        }
        let skill_store = Arc::new(SkillStore::new(storage.clone()));
        let filesystem_skills = Arc::new(FilesystemSkills::with_runtime(
            runtime.workspace(),
            skill_store.clone(),
            runtime.capabilities(),
            skill_dependency_resolver,
        ));
        let skills = Arc::new(SkillRegistry::with_filesystem(
            skill_store,
            filesystem_skills,
        ));
        tools
            .register(SkillSearchTool::new(skills.clone()))
            .expect("register skill_search tool");
        tools
            .register(SkillViewTool::new(skills))
            .expect("register skill_view tool");
        if environment.effective_uid == 0 {
            let broker = Arc::new(SystemAndroidBroker::default());
            tools
                .register(AndroidXiaoStatusTool::new(broker.clone()))
                .expect("register typed Android status tool");
            tools
                .register(AndroidXiaoRestartTool::new(broker))
                .expect("register typed Android restart tool");
        }
        Self::with_registry_runtime(sessions, storage, providers, config, tools, Some(runtime))
    }

    pub fn with_registry(
        sessions: Arc<SessionManager>,
        storage: Arc<Storage>,
        providers: Arc<ProviderRegistry>,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
    ) -> Self {
        Self::with_registry_runtime(sessions, storage, providers, config, tools, None)
    }

    fn with_registry_runtime(
        sessions: Arc<SessionManager>,
        storage: Arc<Storage>,
        providers: Arc<ProviderRegistry>,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        runtime: Option<Arc<RuntimeState>>,
    ) -> Self {
        let memory = Arc::new(if let Some(runtime) = &runtime {
            MemoryStore::with_workspace(storage.clone(), runtime.workspace())
        } else {
            MemoryStore::new(storage.clone())
        });
        let context_engine = if let Some(runtime) = &runtime {
            ContextEngine::with_runtime(storage.clone(), config.clone(), runtime.clone())
        } else {
            ContextEngine::new(storage.clone(), config.clone())
        };
        let memory_evaluator = Arc::new(MemoryEvaluator::new(memory));
        let skill_store = Arc::new(SkillStore::new(storage.clone()));
        let skill_registry = Arc::new(if let Some(runtime) = &runtime {
            SkillRegistry::with_filesystem(
                skill_store.clone(),
                Arc::new(FilesystemSkills::with_runtime(
                    runtime.workspace(),
                    skill_store,
                    runtime.capabilities(),
                    None,
                )),
            )
        } else {
            SkillRegistry::new(skill_store)
        });
        let learning = LearningEvaluator::new(skill_registry, memory_evaluator.clone());
        Self {
            sessions,
            storage,
            providers,
            active: Mutex::new(HashMap::new()),
            tools,
            config,
            memory_evaluator: (*memory_evaluator).clone(),
            context_engine,
            completion: CompletionVerifier,
            learning,
        }
    }

    pub fn cancel(&self, principal: &str) -> bool {
        self.active
            .lock()
            .unwrap()
            .get(principal)
            .map(|t| {
                t.cancel();
                true
            })
            .unwrap_or(false)
    }

    pub async fn submit_with_progress(
        &self,
        principal: &str,
        prompt: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<AgentAnswer> {
        self.run(principal, prompt, true, progress).await
    }

    pub async fn retry_with_progress(
        &self,
        principal: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<AgentAnswer> {
        let ctx = self.sessions.context_for(principal)?;
        let prompt = self
            .storage
            .latest_user_message(principal, &ctx.active.id)?
            .ok_or_else(|| anyhow!("no user request available to retry"))?;
        self.run(principal, &prompt, false, progress).await
    }

    async fn run(
        &self,
        principal: &str,
        prompt: &str,
        append_user: bool,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<AgentAnswer> {
        let token = CancellationToken::new();
        {
            let mut active = self.active.lock().unwrap();
            if active.contains_key(principal) {
                return Err(anyhow!("a generation is already active for this frontend"));
            }
            active.insert(principal.to_owned(), token.clone());
        }

        if append_user {
            if let Err(error) = self.sessions.append_user(principal, prompt) {
                self.active.lock().unwrap().remove(principal);
                return Err(error);
            }
        }

        let ctx = match self.sessions.context_for(principal) {
            Ok(ctx) => ctx,
            Err(error) => {
                self.active.lock().unwrap().remove(principal);
                return Err(error);
            }
        };

        let goal = bound_text(redact_text(prompt), 4_096);
        let agent_run_id = match self.storage.create_agent_run(
            principal,
            &ctx.active.id,
            &ctx.active.provider,
            &ctx.active.model,
            Some(&goal),
        ) {
            Ok(run_id) => run_id,
            Err(error) => {
                self.active.lock().unwrap().remove(principal);
                return Err(error);
            }
        };

        if append_user {
            if let Err(error) =
                self.memory_evaluator
                    .apply_explicit(principal, &ctx.active.id, prompt)
            {
                let safe_error = bound_text(redact_text(&error.to_string()), 4_096);
                let _ = self.storage.set_agent_run_status(
                    principal,
                    &agent_run_id,
                    "failed",
                    Some(&safe_error),
                );
                self.active.lock().unwrap().remove(principal);
                return Err(error);
            }
        }

        let result = async {
            let context = self
                .context_engine
                .build(principal, &ctx, prompt)?
                .messages;
            let provider = self.providers.get(&ctx.active.provider)?;
            let resolved_model = self
                .providers
                .resolve_model(&ctx.active.provider, &ctx.active.model)?;
            if resolved_model != ctx.active.model {
                self.storage.set_session_provider(
                    principal,
                    &ctx.active.id,
                    &ctx.active.provider,
                    ctx.active.account_id.as_deref(),
                    &resolved_model,
                )?;
            }
            self.storage
                .set_agent_run_model(principal, &agent_run_id, &resolved_model)?;
            let (tool_progress_tx, mut tool_progress_rx) = mpsc::unbounded_channel::<String>();
            let progress_relay = progress.clone();
            tokio::spawn(async move {
                while let Some(status) = tool_progress_rx.recv().await {
                    if let Some(tx) = &progress_relay {
                        let _ = tx.send(AgentEvent::Status(status));
                    }
                }
            });
            let tool_context = ToolContext {
                principal: principal.to_owned(),
                session_id: ctx.active.id.clone(),
                agent_run_id: agent_run_id.clone(),
                messages: context.clone(),
                cancellation: token.clone(),
                progress: Some(tool_progress_tx),
            };
            let available_tools = if provider.supports_tool_continuation() {
                self.tools.available_specs(&tool_context)
            } else {
                Vec::new()
            };
            let mut request = ProviderRequest {
                session_id: ctx.active.id.clone(),
                account_id: ctx.active.account_id.clone(),
                model: resolved_model,
                messages: context,
                tools: available_tools,
            };
            let started = AgentEvent::GenerationStarted;
            if let Some(tx) = &progress { let _ = tx.send(started.clone()); }
            let mut continuation=None;
            let mut tool_results=vec![];
            let mut provider_events=vec![];
            let mut artifacts = std::collections::BTreeMap::<std::path::PathBuf, AgentArtifact>::new();
            let execution: Result<LoopOutcome> = async {
            let mut turns = 0usize;
            let mut tool_calls = 0usize;
            let mut failed_actions = std::collections::HashSet::new();
            let mut identical_failure_repeats = 0usize;
            let mut last_unverified_signature = None::<String>;
            let mut last_unverified_evidence = None::<CompletionEvidence>;
            let mut no_progress_repeats = 0usize;
            let run_started = std::time::Instant::now();
            loop {
                if run_started.elapsed().as_secs() >= self.config.max_runtime_seconds {
                    return Err(anyhow!(
                        "agent runtime limit ({} seconds) reached",
                        self.config.max_runtime_seconds
                    ));
                }
                if turns >= self.config.max_turns {
                    if let Some(mut blocked) = last_unverified_evidence {
                        blocked.state = VerificationState::Blocked;
                        blocked.verified = false;
                        blocked.summary = format!(
                            "agent turn limit reached without verification: {}",
                            blocked.summary
                        );
                        break Ok(LoopOutcome {
                            final_answer: format!("Blocked: {}", blocked.summary),
                            verification: blocked,
                        });
                    }
                    return Err(anyhow!(
                        "agent turn limit ({}) reached before a final answer",
                        self.config.max_turns
                    ));
                }
                turns += 1;
                let remaining = std::time::Duration::from_secs(self.config.max_runtime_seconds)
                    .saturating_sub(run_started.elapsed());
                let turn = tokio::select! {
                    _ = token.cancelled() => return Err(anyhow!("generation cancelled")),
                    response = tokio::time::timeout(
                        remaining,
                        provider.run_turn(
                            request.clone(),
                            continuation.take(),
                            tool_results,
                            progress.clone(),
                        ),
                    ) => response.map_err(|_| anyhow!("agent runtime limit reached during provider turn"))??,
                };
                provider_events.extend(turn.events);
                match turn.step {
                    ProviderStep::Final(answer)=>{
                        self.storage.set_agent_run_status(
                            principal,
                            &agent_run_id,
                            "verifying",
                            None,
                        )?;
                        let audit = self.storage.tool_runs(principal, &agent_run_id)?;
                        let verification = self.completion.verify_for_task(prompt, &answer, &audit);
                        match verification.state {
                            VerificationState::VerifiedSuccess => {
                                break Ok(LoopOutcome {
                                    final_answer: answer,
                                    verification,
                                });
                            }
                            VerificationState::Blocked => {
                                break Ok(LoopOutcome {
                                    final_answer: format!("Blocked: {}", verification.summary),
                                    verification,
                                });
                            }
                            VerificationState::Failed => {
                                break Ok(LoopOutcome {
                                    final_answer: answer,
                                    verification,
                                });
                            }
                            VerificationState::NotYetVerified => {
                                last_unverified_evidence = Some(verification.clone());
                                let signature = format!(
                                    "{:?}:{}:{}",
                                    verification.state,
                                    answer.trim(),
                                    audit.iter()
                                        .map(|run| format!("{}:{}:{}", run.tool_name, run.arguments_json, run.status))
                                        .collect::<Vec<_>>()
                                        .join("|")
                                );
                                if last_unverified_signature.as_deref() == Some(&signature) {
                                    no_progress_repeats += 1;
                                } else {
                                    no_progress_repeats = 0;
                                    last_unverified_signature = Some(signature);
                                }
                                if no_progress_repeats >= self.config.max_no_progress_repeats {
                                    let mut blocked = verification;
                                    blocked.state = VerificationState::Blocked;
                                    blocked.verified = false;
                                    blocked.summary = format!(
                                        "bounded no-progress limit reached: {}",
                                        blocked.summary
                                    );
                                    break Ok(LoopOutcome {
                                        final_answer: format!("Blocked: {}", blocked.summary),
                                        verification: blocked,
                                    });
                                }
                                let status = AgentEvent::Status(format!(
                                    "Completion is not verified yet; continuing with observable evidence ({})",
                                    verification.summary
                                ));
                                if let Some(tx) = &progress { let _ = tx.send(status.clone()); }
                                provider_events.push(status);
                                request.messages.push(crate::storage::MessageRecord {
                                    role: "system".into(),
                                    content: format!(
                                        "<COMPLETION_VERIFICATION state=\"{}\">The candidate final answer was not accepted: {}. Continue the task. Observe actual results, choose a materially different action after a failure, and gather independent verification evidence. Do not merely restate that the task is done.</COMPLETION_VERIFICATION>",
                                        match verification.state {
                                            VerificationState::NotYetVerified => "not_yet_verified",
                                            _ => "unknown",
                                        },
                                        verification.summary,
                                    ),
                                    created_at: chrono::Utc::now().to_rfc3339(),
                                });
                                continuation = None;
                                tool_results = Vec::new();
                                self.storage.set_agent_run_status(
                                    principal,
                                    &agent_run_id,
                                    "running",
                                    None,
                                )?;
                            }
                        }
                    },
                    ProviderStep::ToolCalls(calls)=>{
                        if !provider.supports_tool_continuation() {
                            return Err(anyhow!("provider returned tool calls without declaring tool-continuation capability"));
                        }
                        if calls.is_empty(){return Err(anyhow!("provider returned an empty tool-call turn"));}
                        continuation=turn.continuation;
                        let mut next=Vec::with_capacity(calls.len());
                        for call in calls {
                            tool_calls += 1;
                            if tool_calls > self.config.max_tool_calls {
                                let audit = self.storage.tool_runs(principal, &agent_run_id)?;
                                let mut blocked = self.completion.verify_for_task(
                                    prompt,
                                    "tool-call budget exhausted",
                                    &audit,
                                );
                                blocked.state = VerificationState::Blocked;
                                blocked.verified = false;
                                blocked.summary = format!(
                                    "agent tool-call limit ({}) reached without verified success",
                                    self.config.max_tool_calls
                                );
                                return Ok(LoopOutcome {
                                    final_answer: format!("Blocked: {}", blocked.summary),
                                    verification: blocked,
                                });
                            }
                            let started=AgentEvent::ToolStarted(call.name.clone()); if let Some(tx)=&progress{let _=tx.send(started.clone());} provider_events.push(started);
                            let risk = self.tools.spec(&call.name).map(|spec| spec.risk.as_str()).unwrap_or("unknown");
                            let arguments = bounded_json(&redact_json(&call.arguments), 16_384);
                            let redacted_call_id = bound_text(redact_text(&call.call_id), 256);
                            let audit_call_id = if redacted_call_id.trim().is_empty() {
                                format!("malformed-{turns}-{}", next.len())
                            } else {
                                redacted_call_id
                            };
                            let redacted_tool_name = bound_text(redact_text(&call.name), 128);
                            let audit_tool_name = if redacted_tool_name.trim().is_empty() {
                                "unknown".into()
                            } else {
                                redacted_tool_name
                            };
                            let tool_run_id = match self.storage.create_tool_run(
                                &agent_run_id,
                                &audit_call_id,
                                &audit_tool_name,
                                &arguments,
                                risk,
                            ) {
                                Ok(id) => id,
                                Err(error) => {
                                    next.push(ToolResult {
                                        call_id: call.call_id.clone(),
                                        name: call.name.clone(),
                                        output: bound_text(redact_text(&format!("tool was not executed because its audit record could not be created: {error}")), self.config.tool_output_max_chars),
                                        is_error: true,
                                    });
                                    continue;
                                }
                            };
                            let action_signature = format!("{}:{arguments}", call.name);
                            if failed_actions.contains(&action_signature) {
                                identical_failure_repeats += 1;
                                let message = "no-progress guard rejected an identical previously failed action; diagnose the observation and choose a materially different strategy";
                                self.storage.set_tool_run_status(
                                    &tool_run_id,
                                    "denied",
                                    None,
                                    Some(message),
                                )?;
                                let result = ToolResult {
                                    call_id: call.call_id.clone(),
                                    name: call.name.clone(),
                                    output: message.into(),
                                    is_error: true,
                                };
                                let completed = AgentEvent::ToolCompleted {
                                    tool: call.name.clone(),
                                    summary: message.into(),
                                };
                                if let Some(tx) = &progress { let _ = tx.send(completed.clone()); }
                                provider_events.push(completed);
                                next.push(result);
                                if identical_failure_repeats
                                    >= self.config.max_no_progress_repeats
                                {
                                    let audit = self.storage.tool_runs(principal, &agent_run_id)?;
                                    let mut blocked = self.completion.verify_for_task(
                                        prompt,
                                        "no progress",
                                        &audit,
                                    );
                                    blocked.state = VerificationState::Blocked;
                                    blocked.verified = false;
                                    blocked.summary = "bounded no-progress limit reached after repeated identical failed actions".into();
                                    return Ok(LoopOutcome {
                                        final_answer: format!("Blocked: {}", blocked.summary),
                                        verification: blocked,
                                    });
                                }
                                continue;
                            }
                            self.storage.set_tool_run_status(&tool_run_id, "running", None, None)?;
                            let tool_remaining = std::time::Duration::from_secs(self.config.max_runtime_seconds)
                                .saturating_sub(run_started.elapsed());
                            let execution=tokio::select!{
                                _=token.cancelled()=>{
                                    self.storage.set_tool_run_status(
                                        &tool_run_id,
                                        "interrupted",
                                        None,
                                        Some("generation cancelled during tool execution"),
                                    )?;
                                    return Err(anyhow!("generation cancelled"));
                                },
                                _=tokio::time::sleep(tool_remaining)=>{
                                    self.storage.set_tool_run_status(
                                        &tool_run_id,
                                        "interrupted",
                                        None,
                                        Some("agent runtime limit reached during tool execution"),
                                    )?;
                                    return Err(anyhow!("agent runtime limit reached during tool execution"));
                                },
                                result=self.tools.execute(&call,&tool_context)=>result
                            };
                            let result = execution.result;
                            let (output,error) = if result.is_error {
                                (None, Some(result.output.as_str()))
                            } else {
                                (Some(result.output.as_str()), None)
                            };
                            self.storage.set_tool_run_status(
                                &tool_run_id,
                                execution.status.as_str(),
                                output,
                                error,
                            )?;
                            if execution.status == crate::tools::ToolRunStatus::AwaitingApproval {
                                let approval_status = AgentEvent::Status(format!(
                                    "Owner approval required before continuing: {}",
                                    result.output
                                ));
                                if let Some(tx) = &progress { let _ = tx.send(approval_status.clone()); }
                                provider_events.push(approval_status);
                            }
                            if result.is_error {
                                failed_actions.insert(action_signature);
                            } else {
                                for artifact in artifacts_from_tool_output(&result.output) {
                                    artifacts.insert(artifact.path.clone(), artifact);
                                }
                            }
                            let summary=if result.is_error { format!("failed: {}",result.output) } else { "completed".into() };
                            let completed=AgentEvent::ToolCompleted{tool:call.name.clone(),summary}; if let Some(tx)=&progress{let _=tx.send(completed.clone());} provider_events.push(completed);
                            next.push(result);
                        }
                        tool_results=next;
                    }
                }
            }
            }.await;
            let LoopOutcome {
                final_answer,
                verification,
            } = match execution {
                Ok(outcome) => outcome,
                Err(error) => {
                    let status = if token.is_cancelled() || error.to_string().contains("cancelled") {
                        "cancelled"
                    } else {
                        "failed"
                    };
                    let safe_error = bound_text(redact_text(&error.to_string()), 4_096);
                    let _ = self.storage.set_agent_run_status(
                        principal,
                        &agent_run_id,
                        status,
                        Some(&safe_error),
                    );
                    return Err(error);
                }
            };
            if token.is_cancelled() {
                let _ = self.storage.set_agent_run_status(
                    principal,
                    &agent_run_id,
                    "cancelled",
                    Some("generation cancelled before final persistence"),
                );
                return Err(anyhow!("generation cancelled"));
            }
            // Capture the active session before provider execution. A concurrent /session
            // command must never redirect this generation's final write into another session.
            if let Err(error) = self.sessions.append_assistant_to(principal, &ctx.active.id, &final_answer) {
                let safe_error = bound_text(redact_text(&error.to_string()), 4_096);
                let _ = self.storage.set_agent_run_status(principal, &agent_run_id, "failed", Some(&safe_error));
                return Err(error);
            }
            if ctx.mode == ChatMode::Main && ctx.active.name.starts_with("Session ") && ctx.active.message_count <= 1 {
                let title = automatic_title(prompt);
                if !title.is_empty() { let _ = self.storage.rename_session(principal, &ctx.active.id, &title); }
            }
            let tool_audit = self.storage.tool_runs(principal, &agent_run_id)?;
            if verification.state == VerificationState::VerifiedSuccess {
                self.storage
                    .set_agent_run_status(principal, &agent_run_id, "completed", None)?;
                // Feed the complete observable trace to LearningEvaluator.
                // Whether it is reusable is decided from the trace there,
                // rather than from a small verb list in the agent loop.
                let meaningful = verification.task_kind == TaskKind::Action;
                let reusable = meaningful;
                let trace = LearningTrace {
                    run_status: "completed".into(),
                    verified: true,
                    meaningful,
                    reusable,
                    user_goal: bound_text(redact_text(prompt), 2_000),
                    session_id: ctx.active.id.clone(),
                    tool_observations: tool_audit
                        .iter()
                        .take(32)
                        .map(|tool| SafeToolObservation {
                            tool: tool.tool_name.clone(),
                            risk: tool.risk.clone(),
                            status: tool.status.clone(),
                            observable_summary: bound_text(
                                redact_text(
                                    tool.output
                                        .as_deref()
                                        .or(tool.error.as_deref())
                                        .unwrap_or("no output"),
                                ),
                                500,
                            ),
                        })
                        .collect(),
                    final_observable_result: bound_text(
                        redact_text(&final_answer),
                        2_000,
                    ),
                    verification_evidence: verification.summary.clone(),
                    skill_candidate: None,
                };
                // Learning is post-completion and best-effort; failure cannot
                // rewrite a successfully delivered task into a failed one.
                let _ = self.learning.evaluate(principal, &trace);
            } else {
                let status = match verification.state {
                    VerificationState::Blocked => "blocked",
                    VerificationState::Failed | VerificationState::NotYetVerified => "failed",
                    VerificationState::VerifiedSuccess => unreachable!(),
                };
                self.storage.set_agent_run_status(
                    principal,
                    &agent_run_id,
                    status,
                    Some(&verification.summary),
                )?;
            }
            let completed = AgentEvent::GenerationCompleted;
            if let Some(tx) = &progress { let _ = tx.send(completed.clone()); }
            let mut events = vec![started];
            events.extend(provider_events);
            events.push(completed);
            Ok(AgentAnswer {
                progress: events,
                final_answer,
                side_mode: ctx.mode == ChatMode::Side,
                artifacts: artifacts.into_values().collect(),
            })
        }.await;

        self.active.lock().unwrap().remove(principal);
        if let Err(error) = &result {
            let status = if token.is_cancelled() || error.to_string().contains("cancelled") {
                "cancelled"
            } else {
                "failed"
            };
            let safe_error = bound_text(redact_text(&error.to_string()), 4_096);
            let _ = self.storage.set_agent_run_status(
                principal,
                &agent_run_id,
                status,
                Some(&safe_error),
            );
            if let Some(tx) = &progress {
                let _ = tx.send(AgentEvent::GenerationFailed(error.to_string()));
            }
        }
        result
    }
}

fn bound_text(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value
    } else {
        value.chars().take(max_chars).collect::<String>() + "…"
    }
}

fn bounded_json(value: &serde_json::Value, max_chars: usize) -> String {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    if serialized.chars().count() <= max_chars {
        return serialized;
    }
    serde_json::json!({
        "truncated": true,
        "preview": serialized.chars().take(max_chars.saturating_sub(100)).collect::<String>()
    })
    .to_string()
}

fn artifacts_from_tool_output(output: &str) -> Vec<AgentArtifact> {
    serde_json::from_str::<serde_json::Value>(output)
        .ok()
        .and_then(|value| {
            value
                .get("artifacts")
                .and_then(|value| value.as_array())
                .cloned()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

fn automatic_title(prompt: &str) -> String {
    let compact = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = compact.chars().take(52).collect::<String>();
    if compact.chars().count() > 52 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auth::AuthManager,
        providers::{Provider, ProviderResponse, ProviderStep, ProviderTurn},
        tools::{Tool, ToolCall, ToolRisk, ToolSpec},
    };
    use async_trait::async_trait;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    struct SlowProvider;
    #[async_trait]
    impl Provider for SlowProvider {
        fn id(&self) -> &'static str {
            "slow"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        async fn run(
            &self,
            _: ProviderRequest,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(ProviderResponse {
                events: vec![],
                final_answer: "late".into(),
            })
        }
    }

    struct EchoProvider;
    #[async_trait]
    impl Provider for EchoProvider {
        fn id(&self) -> &'static str {
            "echo"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        async fn run(
            &self,
            req: ProviderRequest,
            progress: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            if let Some(tx) = progress {
                let _ = tx.send(AgentEvent::Status("safe status".into()));
            }
            let text = req
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .map(|m| m.content.clone())
                .unwrap_or_default();
            Ok(ProviderResponse {
                events: vec![AgentEvent::Status("safe status".into())],
                final_answer: format!("answer:{text}"),
            })
        }
    }

    struct ToolProvider {
        turns: AtomicUsize,
    }
    #[async_trait]
    impl Provider for ToolProvider {
        fn id(&self) -> &'static str {
            "tool-provider"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn supports_tool_continuation(&self) -> bool {
            true
        }
        async fn run(
            &self,
            _: ProviderRequest,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            Err(anyhow!("run_turn must be used"))
        }
        async fn run_turn(
            &self,
            request: ProviderRequest,
            continuation: Option<serde_json::Value>,
            tool_results: Vec<crate::tools::ToolResult>,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderTurn> {
            let turn = self.turns.fetch_add(1, Ordering::SeqCst);
            if turn == 0 {
                assert!(request
                    .tools
                    .iter()
                    .any(|tool| tool.name == "context_stats"));
                assert!(request.tools.iter().all(|tool| tool.name != "shell"));
                assert!(continuation.is_none());
                assert!(tool_results.is_empty());
                return Ok(ProviderTurn {
                    step: ProviderStep::ToolCalls(vec![ToolCall {
                        call_id: "call-1".into(),
                        name: "context_stats".into(),
                        arguments: serde_json::json!({}),
                    }]),
                    continuation: Some(serde_json::json!({"opaque":"provider-state"})),
                    events: vec![AgentEvent::Status("tool requested".into())],
                });
            }
            assert_eq!(
                continuation
                    .as_ref()
                    .and_then(|v| v.get("opaque"))
                    .and_then(|v| v.as_str()),
                Some("provider-state")
            );
            assert_eq!(tool_results.len(), 1);
            assert_eq!(tool_results[0].name, "context_stats");
            assert!(!tool_results[0].is_error);
            Ok(ProviderTurn {
                step: ProviderStep::Final("tool loop complete".into()),
                continuation: None,
                events: vec![],
            })
        }
    }

    struct LoopProvider {
        turns: AtomicUsize,
    }

    #[async_trait]
    impl Provider for LoopProvider {
        fn id(&self) -> &'static str {
            "loop"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn supports_tool_continuation(&self) -> bool {
            true
        }
        async fn run(
            &self,
            _: ProviderRequest,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            Err(anyhow!("run_turn must be used"))
        }
        async fn run_turn(
            &self,
            _: ProviderRequest,
            _: Option<serde_json::Value>,
            _: Vec<ToolResult>,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderTurn> {
            let turn = self.turns.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderTurn {
                step: ProviderStep::ToolCalls(vec![ToolCall {
                    call_id: format!("loop-{turn}"),
                    name: "context_stats".into(),
                    arguments: serde_json::json!({}),
                }]),
                continuation: None,
                events: vec![],
            })
        }
    }

    struct UnknownToolProvider {
        turns: AtomicUsize,
    }

    #[async_trait]
    impl Provider for UnknownToolProvider {
        fn id(&self) -> &'static str {
            "unknown-tool"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn supports_tool_continuation(&self) -> bool {
            true
        }
        async fn run(
            &self,
            _: ProviderRequest,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            Err(anyhow!("run_turn must be used"))
        }
        async fn run_turn(
            &self,
            _: ProviderRequest,
            _: Option<serde_json::Value>,
            results: Vec<ToolResult>,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderTurn> {
            if self.turns.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(ProviderTurn {
                    step: ProviderStep::ToolCalls(vec![ToolCall {
                        call_id: "bad-call".into(),
                        name: "shell".into(),
                        arguments: serde_json::json!({"cmd":"id"}),
                    }]),
                    continuation: None,
                    events: vec![],
                })
            } else {
                assert_eq!(results.len(), 1);
                assert!(results[0].is_error);
                Ok(ProviderTurn {
                    step: ProviderStep::Final("handled safely".into()),
                    continuation: None,
                    events: vec![],
                })
            }
        }
    }

    struct SlowTool;

    struct AdaptiveActionTool;

    #[async_trait]
    impl Tool for AdaptiveActionTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "adaptive_action".into(),
                description: "Exercise observable failure, changed strategy, and verification in a deterministic test".into(),
                parameters: serde_json::json!({
                    "type":"object",
                    "properties":{"strategy":{"type":"string","enum":["bad","repair","verify"]}},
                    "required":["strategy"],
                    "additionalProperties":false
                }),
                risk: ToolRisk::SideEffect,
                origin: crate::tools::ToolOrigin::Builtin,
                effect: crate::tools::ToolEffect::Idempotent,
                required_capabilities: Vec::new(),
                timeout_ms: 5_000,
            }
        }

        async fn execute(&self, _: &ToolContext, arguments: serde_json::Value) -> Result<String> {
            match arguments.get("strategy").and_then(|value| value.as_str()) {
                Some("bad") => Err(anyhow!("observable first strategy failed")),
                Some("repair") => Ok("{\"changed\":true}".into()),
                Some("verify") => Ok("{\"verified\":true}".into()),
                _ => Err(anyhow!("invalid strategy")),
            }
        }
    }

    struct AdaptiveProvider {
        turns: AtomicUsize,
    }

    #[async_trait]
    impl Provider for AdaptiveProvider {
        fn id(&self) -> &'static str {
            "adaptive"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn supports_tool_continuation(&self) -> bool {
            true
        }
        async fn run(
            &self,
            _: ProviderRequest,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            Err(anyhow!("run_turn must be used"))
        }
        async fn run_turn(
            &self,
            request: ProviderRequest,
            _: Option<serde_json::Value>,
            results: Vec<ToolResult>,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderTurn> {
            let turn = self.turns.fetch_add(1, Ordering::SeqCst);
            let call = |strategy: &str, id: &str| ProviderTurn {
                step: ProviderStep::ToolCalls(vec![ToolCall {
                    call_id: id.into(),
                    name: "adaptive_action".into(),
                    arguments: serde_json::json!({"strategy":strategy}),
                }]),
                continuation: None,
                events: Vec::new(),
            };
            Ok(match turn {
                0 => call("bad", "bad-1"),
                1 => {
                    assert_eq!(results.len(), 1);
                    assert!(results[0].is_error);
                    call("repair", "repair-1")
                }
                2 => {
                    assert_eq!(results.len(), 1);
                    assert!(!results[0].is_error);
                    ProviderTurn {
                        step: ProviderStep::Final("The repair is done.".into()),
                        continuation: None,
                        events: Vec::new(),
                    }
                }
                3 => {
                    assert!(results.is_empty());
                    assert!(request
                        .messages
                        .iter()
                        .any(|message| { message.content.contains("COMPLETION_VERIFICATION") }));
                    call("verify", "verify-1")
                }
                _ => {
                    assert_eq!(results.len(), 1);
                    assert!(results[0].output.contains("verified"));
                    ProviderTurn {
                        step: ProviderStep::Final("The repair is verified.".into()),
                        continuation: None,
                        events: Vec::new(),
                    }
                }
            })
        }
    }

    #[async_trait]
    impl Tool for SlowTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "slow_read".into(),
                description: "Wait in a cancellable read-only test operation".into(),
                parameters: serde_json::json!({
                    "type":"object",
                    "properties":{},
                    "additionalProperties":false
                }),
                risk: ToolRisk::ReadOnly,
                origin: crate::tools::ToolOrigin::Builtin,
                effect: crate::tools::ToolEffect::None,
                required_capabilities: Vec::new(),
                timeout_ms: 30_000,
            }
        }
        async fn execute(&self, _: &ToolContext, _: serde_json::Value) -> Result<String> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok("late".into())
        }
    }

    struct SlowToolProvider;

    #[async_trait]
    impl Provider for SlowToolProvider {
        fn id(&self) -> &'static str {
            "slow-tool-provider"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn supports_tool_continuation(&self) -> bool {
            true
        }
        async fn run(
            &self,
            _: ProviderRequest,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            Err(anyhow!("run_turn must be used"))
        }
        async fn run_turn(
            &self,
            _: ProviderRequest,
            continuation: Option<serde_json::Value>,
            _: Vec<ToolResult>,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderTurn> {
            if continuation.is_none() {
                Ok(ProviderTurn {
                    step: ProviderStep::ToolCalls(vec![ToolCall {
                        call_id: "slow-1".into(),
                        name: "slow_read".into(),
                        arguments: serde_json::json!({}),
                    }]),
                    continuation: Some(serde_json::json!({"after":true})),
                    events: vec![],
                })
            } else {
                Ok(ProviderTurn {
                    step: ProviderStep::Final("late".into()),
                    continuation: None,
                    events: vec![],
                })
            }
        }
    }

    struct GateProvider {
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Provider for GateProvider {
        fn id(&self) -> &'static str {
            "gate"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        async fn run(
            &self,
            _: ProviderRequest,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(ProviderResponse {
                events: vec![],
                final_answer: "captured target".into(),
            })
        }
    }

    fn engine(
        provider_id: &str,
        provider: Arc<dyn Provider>,
    ) -> (Arc<AgentEngine>, Arc<Storage>, String, tempfile::TempDir) {
        let db = Arc::new(Storage::open_memory().unwrap());
        let sessions = Arc::new(SessionManager::new(db.clone()));
        let main = sessions.ensure_default_session("u").unwrap();
        db.set_session_provider("u", &main.id, provider_id, None, "m")
            .unwrap();
        sessions.switch_main("u", &main.id).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let auth = Arc::new(AuthManager::new(db.clone(), tmp.path().join("secrets")));
        let providers = Arc::new(ProviderRegistry::from_single(provider_id, provider, auth));
        (
            Arc::new(AgentEngine::new(sessions, db.clone(), providers)),
            db,
            main.id,
            tmp,
        )
    }

    #[tokio::test]
    async fn stop_cancels_active_generation() {
        let (engine, db, session, _tmp) = engine("slow", Arc::new(SlowProvider));
        let running = engine.clone();
        let task =
            tokio::spawn(async move { running.submit_with_progress("u", "hello", None).await });
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(engine.cancel("u"));
        let error = task.await.unwrap().unwrap_err().to_string();
        assert!(error.contains("cancelled"));
        assert_eq!(db.messages("u", &session).unwrap().len(), 1);
        assert_eq!(db.agent_runs("u", 10).unwrap()[0].status, "cancelled");
    }

    #[tokio::test]
    async fn retry_reuses_latest_user_without_duplicate_user_row() {
        let (engine, db, session, _tmp) = engine("echo", Arc::new(EchoProvider));
        engine
            .submit_with_progress("u", "hello", None)
            .await
            .unwrap();
        engine.retry_with_progress("u", None).await.unwrap();
        let messages = db.messages("u", &session).unwrap();
        assert_eq!(messages.iter().filter(|m| m.role == "user").count(), 1);
        assert_eq!(messages.iter().filter(|m| m.role == "assistant").count(), 2);
    }

    #[tokio::test]
    async fn progress_channel_contains_only_safe_status_events() {
        let (engine, _, _, _tmp) = engine("echo", Arc::new(EchoProvider));
        let (tx, mut rx) = mpsc::unbounded_channel();
        engine
            .submit_with_progress("u", "hello", Some(tx))
            .await
            .unwrap();
        let mut events = vec![];
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::Status(s) if s == "safe status")));
    }

    #[tokio::test]
    async fn typed_tool_call_continues_provider_until_final_answer() {
        let provider = Arc::new(ToolProvider {
            turns: AtomicUsize::new(0),
        });
        let (engine, db, session, _tmp) = engine("tool-provider", provider.clone());
        let answer = engine
            .submit_with_progress("u", "use the safe tool", None)
            .await
            .unwrap();
        assert_eq!(answer.final_answer, "tool loop complete");
        assert_eq!(provider.turns.load(Ordering::SeqCst), 2);
        assert!(answer
            .progress
            .iter()
            .any(|e| matches!(e,AgentEvent::ToolStarted(name) if name=="context_stats")));
        assert!(answer
            .progress
            .iter()
            .any(|e| matches!(e,AgentEvent::ToolCompleted{tool,..} if tool=="context_stats")));
        let messages = db.messages("u", &session).unwrap();
        assert_eq!(messages.last().unwrap().content, "tool loop complete");
        let runs = db.agent_runs("u", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "completed");
        let tool_runs = db.tool_runs("u", &runs[0].id).unwrap();
        assert_eq!(tool_runs.len(), 1);
        assert_eq!(tool_runs[0].status, "succeeded");
        assert_eq!(tool_runs[0].tool_name, "context_stats");
    }

    #[tokio::test]
    async fn max_turn_guard_fails_run_without_persisting_assistant() {
        let provider = Arc::new(LoopProvider {
            turns: AtomicUsize::new(0),
        });
        let (engine, db, session, _tmp) = engine("loop", provider.clone());
        let config = AgentConfig {
            max_turns: 2,
            ..AgentConfig::default()
        };
        let bounded = Arc::new(AgentEngine::with_config(
            engine.sessions.clone(),
            db.clone(),
            engine.providers.clone(),
            config,
        ));
        let error = bounded
            .submit_with_progress("u", "loop forever", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("turn limit"));
        assert_eq!(provider.turns.load(Ordering::SeqCst), 2);
        assert_eq!(db.messages("u", &session).unwrap().len(), 1);
        let run = &db.agent_runs("u", 10).unwrap()[0];
        assert_eq!(run.status, "failed");
        assert_eq!(db.tool_runs("u", &run.id).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn unknown_tool_is_a_durable_denied_observation_not_a_crash() {
        let provider = Arc::new(UnknownToolProvider {
            turns: AtomicUsize::new(0),
        });
        let (engine, db, _, _tmp) = engine("unknown-tool", provider);
        let answer = engine
            .submit_with_progress("u", "try an unknown tool", None)
            .await
            .unwrap();
        assert_eq!(answer.final_answer, "handled safely");
        let run = &db.agent_runs("u", 10).unwrap()[0];
        assert_eq!(run.status, "failed");
        let tools = db.tool_runs("u", &run.id).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].status, "denied");
        assert_eq!(tools[0].risk, "unknown");
    }

    #[tokio::test]
    async fn cancellation_during_tool_marks_both_run_boundaries_terminal() {
        let (base, db, session, _tmp) = engine("slow-tool-provider", Arc::new(SlowToolProvider));
        let registry = Arc::new(ToolRegistry::new(ToolPolicy::default(), 4_096));
        registry.register(SlowTool).unwrap();
        let engine = Arc::new(AgentEngine::with_registry(
            base.sessions.clone(),
            db.clone(),
            base.providers.clone(),
            AgentConfig::default(),
            registry,
        ));
        let running = engine.clone();
        let task = tokio::spawn(async move {
            running
                .submit_with_progress("u", "run slow tool", None)
                .await
        });
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(engine.cancel("u"));
        assert!(task
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
        assert_eq!(db.messages("u", &session).unwrap().len(), 1);
        let run = &db.agent_runs("u", 10).unwrap()[0];
        assert_eq!(run.status, "cancelled");
        assert_eq!(db.tool_runs("u", &run.id).unwrap()[0].status, "interrupted");
    }

    #[tokio::test]
    async fn failure_changes_strategy_and_unverified_final_continues_until_evidence() {
        let provider = Arc::new(AdaptiveProvider {
            turns: AtomicUsize::new(0),
        });
        let (base, db, session, _tmp) = engine("adaptive", provider.clone());
        let registry = Arc::new(ToolRegistry::new(
            ToolPolicy::default().allow_side_effect("adaptive_action"),
            4_096,
        ));
        registry.register(AdaptiveActionTool).unwrap();
        let engine = AgentEngine::with_registry(
            base.sessions.clone(),
            db.clone(),
            base.providers.clone(),
            AgentConfig::default(),
            registry,
        );
        let answer = engine
            .submit_with_progress("u", "Fix the reusable widget workflow", None)
            .await
            .unwrap();
        assert_eq!(answer.final_answer, "The repair is verified.");
        assert_eq!(provider.turns.load(Ordering::SeqCst), 5);
        let messages = db.messages("u", &session).unwrap();
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.role == "assistant")
                .count(),
            1
        );
        let run = &db.agent_runs("u", 10).unwrap()[0];
        assert_eq!(run.status, "completed");
        let tools = db.tool_runs("u", &run.id).unwrap();
        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.status.as_str())
                .collect::<Vec<_>>(),
            ["failed", "succeeded", "succeeded"]
        );
        assert_ne!(tools[0].arguments_json, tools[1].arguments_json);
        let learned = crate::skills::SkillStore::new(db).list("u", 10).unwrap();
        assert_eq!(learned.len(), 1);
        assert!(learned[0].procedure.contains("materially different"));
        assert!(learned[0].pitfalls.contains("first strategy failed"));
    }

    #[tokio::test]
    async fn concurrent_session_switch_cannot_redirect_captured_final_write() {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (engine, db, captured_session, _tmp) = engine(
            "gate",
            Arc::new(GateProvider {
                started: started.clone(),
                release: release.clone(),
            }),
        );
        let running = engine.clone();
        let task =
            tokio::spawn(
                async move { running.submit_with_progress("u", "keep target", None).await },
            );
        started.notified().await;
        let switched = engine.sessions.create_and_switch("u").unwrap();
        assert_ne!(captured_session, switched.id);
        release.notify_one();
        task.await.unwrap().unwrap();
        let captured = db.messages("u", &captured_session).unwrap();
        assert_eq!(captured.last().unwrap().content, "captured target");
        assert!(db.messages("u", &switched.id).unwrap().is_empty());
    }

    #[test]
    fn automatic_title_is_short_and_single_line() {
        let title = automatic_title(
            "  Build   a\nsmall Telegram gateway with safe callbacks and persistence please ",
        );
        assert!(!title.contains('\n'));
        assert!(title.chars().count() <= 53);
    }
}
