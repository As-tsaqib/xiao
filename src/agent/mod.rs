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
    attachments::AttachmentManager,
    config::AgentConfig,
    context::{ContextEngine, SessionHistoryStore},
    learning::{LearningTrace, SafeToolObservation},
    memory::MemoryStore,
    providers::{
        AgentEvent, ProviderPdfFallback, ProviderRegistry, ProviderRequest, ProviderStep,
        ToolProtocol,
    },
    runtime::{
        DependencyResolver, ProcessExecutor, RuntimeState, SystemAndroidBroker, TermuxExecutor,
        TermuxPackageBackend, TermuxRepositoryBackend,
    },
    security::redact::{redact_json, redact_text},
    semantic::SemanticEvaluator,
    session::{ChatMode, SessionManager},
    skills::{FilesystemSkills, SkillRegistry, SkillStore},
    storage::Storage,
    telegram::TelegramScope,
    tools::{
        builtin::{
            AndroidXiaoRestartTool, AndroidXiaoStatusTool, ContextStatsTool, MemoryDeleteTool,
            MemorySearchTool, MemorySetTool, SkillSearchTool, SkillViewTool, TermuxJobTool,
            TermuxTerminalTool,
        },
        ToolContext, ToolPolicy, ToolRegistry, ToolResult,
    },
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentAnswer {
    pub run_id: String,
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
    config: Arc<tokio::sync::RwLock<AgentConfig>>,
    context_engine: ContextEngine,
    attachments: Option<Arc<AttachmentManager>>,
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
        tools
            .register(crate::tools::builtin::PdfCreateTool::new("/workspace"))
            .expect("register pdf_create tool");
        tools
            .register_alias("create_pdf", "pdf_create")
            .expect("register create_pdf alias");
        Self::with_registry(sessions, storage, providers, config, tools)
    }

    pub fn with_runtime(
        sessions: Arc<SessionManager>,
        storage: Arc<Storage>,
        providers: Arc<ProviderRegistry>,
        config: AgentConfig,
        runtime: Arc<RuntimeState>,
        attachments: Option<Arc<AttachmentManager>>,
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
                termux.clone(),
                termux_home.clone(),
            ));
            let repository = Arc::new(TermuxRepositoryBackend::new(
                executor.clone(),
                &termux,
                termux_home.clone(),
            ));
            let resolver = Arc::new(DependencyResolver::with_trusted_repository(
                runtime.capabilities(),
                package_backend,
                Some(storage.clone()),
                repository,
            ));
            skill_dependency_resolver = Some(resolver.clone());
            let terminal = TermuxTerminalTool::new(executor, resolver, termux_home);
            tools
                .register(TermuxJobTool::with_storage(
                    terminal.clone(),
                    config.max_execution_plan_steps,
                    storage.clone(),
                ))
                .expect("register termux_job tool");
            tools
                .register(terminal)
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
        let default_pdf_cwd = if let Some(termux) = environment.termux.as_ref() {
            termux.home.clone()
        } else {
            runtime.workspace().root().to_path_buf()
        };
        tools
            .register(crate::tools::builtin::PdfCreateTool::new(default_pdf_cwd))
            .expect("register pdf_create tool");
        tools
            .register_alias("create_pdf", "pdf_create")
            .expect("register create_pdf alias");
        if environment.effective_uid == 0 {
            let broker = Arc::new(SystemAndroidBroker::default());
            tools
                .register(AndroidXiaoStatusTool::new(broker.clone()))
                .expect("register typed Android status tool");
            tools
                .register(AndroidXiaoRestartTool::new(broker))
                .expect("register typed Android restart tool");
        }
        Self::with_registry_runtime(
            sessions,
            storage,
            providers,
            config,
            tools,
            Some(runtime),
            attachments,
        )
    }

    pub fn with_registry(
        sessions: Arc<SessionManager>,
        storage: Arc<Storage>,
        providers: Arc<ProviderRegistry>,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
    ) -> Self {
        Self::with_registry_runtime(sessions, storage, providers, config, tools, None, None)
    }

    fn with_registry_runtime(
        sessions: Arc<SessionManager>,
        storage: Arc<Storage>,
        providers: Arc<ProviderRegistry>,
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        runtime: Option<Arc<RuntimeState>>,
        attachments: Option<Arc<AttachmentManager>>,
    ) -> Self {
        let context_engine = if let Some(runtime) = &runtime {
            ContextEngine::with_runtime_and_attachments(
                storage.clone(),
                config.clone(),
                runtime.clone(),
                attachments.clone(),
            )
        } else if let Some(attachments) = &attachments {
            ContextEngine::with_attachments(storage.clone(), config.clone(), attachments.clone())
        } else {
            ContextEngine::new(storage.clone(), config.clone())
        };
        Self {
            sessions,
            storage,
            providers,
            active: Mutex::new(HashMap::new()),
            tools,
            config: Arc::new(tokio::sync::RwLock::new(config)),
            context_engine,
            attachments,
        }
    }

    async fn execute_readonly_group(
        &self,
        calls: Vec<crate::tools::ToolCall>,
        context: &ToolContext,
        run_id: &str,
        limit: usize,
        progress: Option<&mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<Vec<ToolResult>> {
        let mut prepared = Vec::with_capacity(calls.len());
        for call in calls {
            let arguments = bounded_json(&redact_json(&call.arguments), 16_384);
            let id = self.storage.create_tool_run(
                run_id,
                &bound_text(redact_text(&call.call_id), 256),
                &bound_text(redact_text(&call.name), 128),
                &arguments,
                "read_only",
            )?;
            self.storage
                .set_tool_run_status(&id, "running", None, None)?;
            let event = AgentEvent::ToolStartedWithId {
                tool: call.name.clone(),
                call_id: call.call_id.clone(),
            };
            if let Some(progress) = progress {
                let _ = progress.send(event);
            }
            prepared.push((call, id));
        }
        let ids = prepared
            .iter()
            .map(|(_, id)| id.clone())
            .collect::<Vec<_>>();
        let calls = prepared.into_iter().map(|(call, _)| call).collect();
        let executions = crate::tools::scheduler::schedule(
            calls,
            true,
            limit,
            |_| crate::tools::scheduler::ToolExecutionClass::ReadOnlyParallelSafe,
            |call| async move { self.tools.execute(&call, context).await },
        )
        .await;
        let mut results = Vec::with_capacity(executions.len());
        for (id, execution) in ids.into_iter().zip(executions) {
            let result = execution.result;
            let interrupted = context.cancellation.is_cancelled();
            self.storage.set_tool_run_status(
                &id,
                if interrupted {
                    "interrupted"
                } else {
                    execution.status.as_str()
                },
                (!result.is_error).then_some(result.output.as_str()),
                result.is_error.then_some(result.output.as_str()),
            )?;
            if let Some(progress) = progress {
                let _ = progress.send(AgentEvent::ToolCompletedWithId {
                    tool: result.name.clone(),
                    call_id: result.call_id.clone(),
                    summary: if interrupted {
                        "cancelled".into()
                    } else if result.is_error {
                        format!("failed: {}", result.output)
                    } else {
                        "completed".into()
                    },
                });
            }
            results.push(result);
        }
        if context.cancellation.is_cancelled() {
            return Err(anyhow!("generation cancelled during parallel tool group"));
        }
        Ok(results)
    }

    pub fn cancel(&self, principal: &str) -> bool {
        self.cancel_in_scope(principal, None)
    }

    pub async fn update_config(&self, config: AgentConfig) {
        *self.config.write().await = config;
    }

    pub fn reload_config(&self, config: AgentConfig) {
        if let Ok(mut lock) = self.config.try_write() {
            *lock = config;
        } else {
            let conf_lock = self.config.clone();
            tokio::spawn(async move {
                *conf_lock.write().await = config;
            });
        }
    }

    pub fn cancel_in_scope(&self, principal: &str, scope: Option<TelegramScope>) -> bool {
        self.active
            .lock()
            .unwrap()
            .get(&run_key(principal, scope, None))
            .map(|t| {
                t.cancel();
                true
            })
            .unwrap_or(false)
    }

    pub fn is_active_in_scope(&self, principal: &str, scope: Option<TelegramScope>) -> bool {
        self.active
            .lock()
            .map(|active| active.contains_key(&run_key(principal, scope, None)))
            .unwrap_or(false)
    }

    pub fn cancel_session(&self, principal: &str, session_id: &str) -> bool {
        self.active
            .lock()
            .unwrap()
            .get(&run_key(principal, None, Some(session_id)))
            .map(|token| {
                token.cancel();
                true
            })
            .unwrap_or(false)
    }

    pub fn is_active_session(&self, principal: &str, session_id: &str) -> bool {
        self.active
            .lock()
            .map(|active| active.contains_key(&run_key(principal, None, Some(session_id))))
            .unwrap_or(false)
    }

    pub async fn submit_with_progress(
        &self,
        principal: &str,
        prompt: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<AgentAnswer> {
        self.run(principal, None, None, prompt, true, progress, None)
            .await
    }

    pub async fn submit_with_progress_in_scope(
        &self,
        principal: &str,
        scope: TelegramScope,
        prompt: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<AgentAnswer> {
        self.run(principal, Some(scope), None, prompt, true, progress, None)
            .await
    }

    /// Run a Telegram request under the caller's cancellation lineage.  The
    /// adapter uses one parent token for download, ingestion, provider work
    /// and tools so `/stop` can interrupt the whole message work item.
    pub async fn submit_with_progress_in_scope_with_cancellation(
        &self,
        principal: &str,
        scope: TelegramScope,
        prompt: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
        cancellation: CancellationToken,
    ) -> Result<AgentAnswer> {
        self.run(
            principal,
            Some(scope),
            None,
            prompt,
            true,
            progress,
            Some(cancellation),
        )
        .await
    }

    pub async fn submit_to_session_with_progress(
        &self,
        principal: &str,
        session_id: &str,
        prompt: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<AgentAnswer> {
        self.run(
            principal,
            None,
            Some(session_id),
            prompt,
            true,
            progress,
            None,
        )
        .await
    }

    pub async fn retry_to_session_with_progress(
        &self,
        principal: &str,
        session_id: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<AgentAnswer> {
        let ctx = self.sessions.context_for_session(principal, session_id)?;
        let prompt = self
            .storage
            .latest_user_message(principal, &ctx.active.id)?
            .ok_or_else(|| anyhow!("no user request available to retry"))?;
        self.run(
            principal,
            None,
            Some(session_id),
            &prompt,
            false,
            progress,
            None,
        )
        .await
    }

    pub async fn retry_with_progress(
        &self,
        principal: &str,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<AgentAnswer> {
        self.retry_with_progress_in_scope(principal, None, progress)
            .await
    }

    pub async fn retry_with_progress_in_scope(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<AgentAnswer> {
        let ctx = match scope {
            Some(scope) => self.sessions.context_for_telegram(principal, scope)?,
            None => self.sessions.context_for(principal)?,
        };
        let prompt = self
            .storage
            .latest_user_message(principal, &ctx.active.id)?
            .ok_or_else(|| anyhow!("no user request available to retry"))?;
        self.run(principal, scope, None, &prompt, false, progress, None)
            .await
    }

    pub async fn retry_with_progress_in_scope_with_cancellation(
        &self,
        principal: &str,
        scope: TelegramScope,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
        cancellation: CancellationToken,
    ) -> Result<AgentAnswer> {
        let ctx = self.sessions.context_for_telegram(principal, scope)?;
        let prompt = self
            .storage
            .latest_user_message(principal, &ctx.active.id)?
            .ok_or_else(|| anyhow!("no user request available to retry"))?;
        self.run(
            principal,
            Some(scope),
            None,
            &prompt,
            false,
            progress,
            Some(cancellation),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        principal: &str,
        scope: Option<TelegramScope>,
        explicit_session: Option<&str>,
        prompt: &str,
        append_user: bool,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
        parent_cancellation: Option<CancellationToken>,
    ) -> Result<AgentAnswer> {
        // One immutable settings snapshot governs this run; WebUI updates are
        // visible only to later runs.
        let config = self.config.read().await.clone();
        let token = parent_cancellation
            .map(|parent| parent.child_token())
            .unwrap_or_default();
        let active_key = run_key(principal, scope, explicit_session);
        {
            let mut active = self.active.lock().unwrap();
            if active.contains_key(&active_key) {
                return Err(anyhow!("a generation is already active for this frontend"));
            }
            active.insert(active_key.clone(), token.clone());
        }

        let ctx = if let Some(session_id) = explicit_session {
            self.sessions.context_for_session(principal, session_id)
        } else {
            match scope {
                Some(scope) => self.sessions.context_for_telegram(principal, scope),
                None => self.sessions.context_for(principal),
            }
        };
        let ctx = match ctx {
            Ok(ctx) => ctx,
            Err(error) => {
                self.active.lock().unwrap().remove(&active_key);
                return Err(error);
            }
        };

        let active_provider = match self.storage.active_provider_kind() {
            Ok(provider) if provider == "custom" => provider,
            Ok(_) => {
                self.active.lock().unwrap().remove(&active_key);
                return Err(anyhow!("provider runtime policy is invalid"));
            }
            Err(error) => {
                self.active.lock().unwrap().remove(&active_key);
                return Err(anyhow!("provider runtime policy is unavailable: {error}"));
            }
        };
        if ctx.active.provider != active_provider {
            self.active.lock().unwrap().remove(&active_key);
            return Err(anyhow!(
                "provider_configuration_required: session uses legacy provider '{}'; select a supported Custom profile and exact model before generating",
                ctx.active.provider
            ));
        }
        let provider = match self.providers.get(&ctx.active.provider) {
            Ok(provider) => provider,
            Err(error) => {
                self.active.lock().unwrap().remove(&active_key);
                return Err(error);
            }
        };
        let resolved_model = match self.providers.resolve_model_for(
            &ctx.active.provider,
            &ctx.active.model,
            ctx.active.account_id.as_deref(),
        ) {
            Ok(model) => model,
            Err(error) => {
                self.active.lock().unwrap().remove(&active_key);
                return Err(error);
            }
        };
        // Resolve and validate the captured session before recording a new
        // request. A legacy Codex/Antigravity history stays readable, but a
        // rejected generation must not mutate it with an unserviceable user
        // message. Writing to the captured id also prevents a concurrent UI
        // session switch from redirecting this request.
        if append_user {
            if let Err(error) =
                self.sessions
                    .append_user_to_session(principal, &ctx.active.id, prompt)
            {
                self.active.lock().unwrap().remove(&active_key);
                return Err(error);
            }
        }
        let semantic = Arc::new(
            if provider
                .supports_semantic_evaluation_for(&resolved_model, ctx.active.account_id.as_deref())
            {
                SemanticEvaluator::with_provider(
                    provider.clone(),
                    ctx.active.id.clone(),
                    ctx.active.account_id.clone(),
                    resolved_model.clone(),
                )
            } else {
                SemanticEvaluator::deterministic()
            },
        );
        let completion = CompletionVerifier::with_semantic(semantic);

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
                self.active.lock().unwrap().remove(&active_key);
                return Err(error);
            }
        };
        let timing_started = std::time::Instant::now();

        let result = async {
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
            let provider_capabilities = provider.capabilities_for(
                &resolved_model,
                ctx.active.account_id.as_deref(),
            );
            let images = if let Some(attachments) = &self.attachments {
                let referenced = attachments.recent_for_prompt(
                    principal,
                    &ctx.active.id,
                    prompt,
                    4,
                )?;
                for document in referenced.iter().filter(|attachment| {
                    attachment.detected_mime == "application/pdf"
                        && matches!(
                            attachment.processing_status.as_str(),
                            "needs_ocr" | "blocked"
                        )
                }) {
                    let pdf_provider = ProviderPdfFallback::new(provider.as_ref());
                    let path = attachments
                        .process_pending_pdf_with_fallback(
                            principal,
                            &ctx.active.id,
                            &document.attachment_id,
                            prompt,
                            &ctx.active.provider,
                            ctx.active.account_id.as_deref(),
                            &resolved_model,
                            provider.pdf_fallback_capabilities(
                                &resolved_model,
                                ctx.active.account_id.as_deref(),
                            ),
                            &pdf_provider,
                            &token,
                        )
                        .await?;
                    if matches!(path, crate::attachments::PdfProcessingPath::Blocked) {
                        let detail = attachments
                            .recent_for_prompt(principal, &ctx.active.id, prompt, 4)?
                            .into_iter()
                            .find(|item| item.attachment_id == document.attachment_id)
                            .and_then(|item| item.error.or(item.summary))
                            .unwrap_or_else(|| {
                                "no explicit local OCR, file-input, or vision path succeeded"
                                    .into()
                            });
                        return Err(anyhow!(
                            "{} could not be processed safely: {}. Xiao will not pretend the document was read.",
                            document.original_name,
                            detail
                        ));
                    }
                }
                let referenced = attachments.recent_for_prompt(
                    principal,
                    &ctx.active.id,
                    prompt,
                    4,
                )?;
                if let Some(document) = referenced
                    .iter()
                    .find(|attachment| attachment.processing_status != "ready")
                {
                    return Err(anyhow!(
                        "{} is not ready for safe processing (status: {}). Xiao will not pretend the document was read.",
                        document.original_name,
                        document.processing_status
                    ));
                }
                let images = attachments.normalized_images(principal, &ctx.active.id, prompt)?;
                if !images.is_empty() && !provider_capabilities.vision {
                    return Err(anyhow!(
                        "selected provider/model does not declare vision capability. Switch to a vision-capable model in /model before asking Xiao to inspect this image"
                    ));
                }
                images
            } else {
                Vec::new()
            };
            // A scanned PDF may have been admitted as `needs_ocr` and completed
            // through provider file/vision fallback above. Build the provider
            // context after that planner so the same turn can retrieve the
            // newly indexed extracted text instead of requiring a follow-up
            // prompt.
            let context = self
                .context_engine
                .build(principal, &ctx, prompt)?
                .messages;
            let tool_context = ToolContext {
                principal: principal.to_owned(),
                session_id: ctx.active.id.clone(),
                agent_run_id: agent_run_id.clone(),
                yolo_mode: ctx.active.yolo_mode,
                messages: context.clone(),
                cancellation: token.clone(),
                progress: Some(tool_progress_tx),
            };
            let task_kind = tokio::select! {
                _ = token.cancelled() => return Err(anyhow!("generation cancelled during task classification")),
                kind = completion.classify_async(prompt, &[]) => kind,
            };
            if provider_capabilities.tool_protocol == ToolProtocol::ChatOnly
                && task_kind != TaskKind::Informational
            {
                return Err(anyhow!(
                    "selected provider/model is explicitly ChatOnly and cannot safely execute this action task: {}",
                    provider_capabilities.evidence
                ));
            }
            let available_tools = if provider_capabilities.is_agent_capable() {
                let all_specs = self.tools.available_specs(&tool_context);
                let filtered: Vec<_> = if task_kind == TaskKind::Informational {
                    all_specs
                        .into_iter()
                        .filter(|spec| {
                            spec.risk == crate::tools::ToolRisk::ReadOnly
                                || matches!(
                                    spec.name.as_str(),
                                    "session_search" | "memory" | "recall" | "context_stats" | "skills"
                                )
                        })
                        .collect()
                } else {
                    all_specs
                };
                if config.execution_plan_enabled {
                    filtered
                } else {
                    filtered
                        .into_iter()
                        .filter(|spec| spec.name != "termux_job")
                        .collect()
                }
            } else {
                Vec::new()
            };
            let mut request = ProviderRequest {
                session_id: ctx.active.id.clone(),
                account_id: ctx.active.account_id.clone(),
                model: resolved_model,
                messages: context,
                tools: available_tools,
                images,
                files: Vec::new(),
                streaming: config.provider_streaming,
            };
            self.storage.record_agent_run_event(&agent_run_id, "pre_provider_overhead", timing_started.elapsed().as_millis() as u64, &serde_json::json!({}))?;
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
            let mut observation_signatures = std::collections::VecDeque::<String>::new();
            let run_started = std::time::Instant::now();
            loop {
                compact_provider_messages(&mut request.messages, config.context_max_chars, config.summary_threshold_chars);
                if run_started.elapsed().as_secs() >= config.max_runtime_seconds {
                    return Err(anyhow!(
                        "agent runtime limit ({} seconds) reached",
                        config.max_runtime_seconds
                    ));
                }
                if turns >= config.max_turns {
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
                    let last_state = request
                        .messages
                        .iter()
                        .rev()
                        .find(|m| m.role == "system" || m.role == "tool")
                        .map(|m| format!("; last observable state: {}", m.content))
                        .unwrap_or_default();
                    return Err(anyhow!(
                        "agent turn limit ({}) reached before a final answer{}",
                        config.max_turns,
                        last_state
                    ));
                }
                turns += 1;
                self.storage.record_agent_run_event(&agent_run_id, "provider_request_start", timing_started.elapsed().as_millis() as u64, &serde_json::json!({"turn":turns}))?;
                let remaining = std::time::Duration::from_secs(config.max_runtime_seconds)
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
                self.storage.record_agent_run_event(&agent_run_id, "provider_completion", timing_started.elapsed().as_millis() as u64, &serde_json::json!({"turn":turns}))?;
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
                        let has_images = !request.images.is_empty();
                        let verification = tokio::select! {
                            _ = token.cancelled() => return Err(anyhow!("generation cancelled during completion verification")),
                            evidence = completion.verify_for_task_with_images_async(prompt, &answer, &audit, has_images) => evidence,
                        };
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
                                    "{:?}:{}",
                                    verification.state,
                                    audit.iter()
                                        .map(|run| format!(
                                            "{}:{}:{}:{}",
                                            run.tool_name,
                                            run.arguments_json,
                                            run.status,
                                            run.output.as_deref().or(run.error.as_deref()).unwrap_or_default()
                                        ))
                                        .collect::<Vec<_>>()
                                        .join("|")
                                );
                                if last_unverified_signature.as_deref() == Some(&signature) {
                                    no_progress_repeats += 1;
                                } else {
                                    no_progress_repeats = 0;
                                    last_unverified_signature = Some(signature);
                                }
                                if no_progress_repeats >= config.max_no_progress_repeats {
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
                                let installs = self
                                    .storage
                                    .dependency_installs(&agent_run_id)
                                    .unwrap_or_default();
                                let observations = run_observations_block(
                                    prompt,
                                    &verification,
                                    &audit,
                                    &installs,
                                    artifacts.values(),
                                    turns,
                                    config.max_turns.saturating_sub(turns),
                                    config.max_tool_calls.saturating_sub(tool_calls),
                                    remaining.as_secs(),
                                );
                                request.messages.push(crate::storage::MessageRecord {
                                    role: "system".into(),
                                    content: observations,
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
                        if provider_capabilities.tool_protocol == ToolProtocol::ChatOnly {
                            return Err(anyhow!("ChatOnly provider returned tool calls in violation of its declared capability"));
                        }
                        if calls.is_empty(){return Err(anyhow!("provider returned an empty tool-call turn"));}
                        continuation=turn.continuation;
                        if config.parallel_readonly_tools && calls.len()>1 && calls.iter().all(|call| crate::tools::scheduler::execution_class(self.tools.spec(&call.name).as_ref())==crate::tools::scheduler::ToolExecutionClass::ReadOnlyParallelSafe) {
                            if tool_calls.saturating_add(calls.len())>config.max_tool_calls { return Err(anyhow!("agent tool-call limit ({}) reached",config.max_tool_calls)); }
                            tool_calls+=calls.len();
                            let group_started=timing_started.elapsed().as_millis() as u64;
                            tool_results=self.execute_readonly_group(calls,&tool_context,&agent_run_id,config.max_parallel_readonly_tools,progress.as_ref()).await?;
                            self.storage.record_agent_run_event(&agent_run_id,"tool_group",timing_started.elapsed().as_millis() as u64,&serde_json::json!({"started_ms":group_started,"class":"parallel_read_only","count":tool_results.len()}))?;
                            continue;
                        }
                        let mut next=Vec::with_capacity(calls.len());
                        for call in calls {
                            tool_calls += 1;
                            if tool_calls > config.max_tool_calls {
                                let audit = self.storage.tool_runs(principal, &agent_run_id)?;
                                let mut blocked = completion
                                    .verify_for_task_async(
                                        prompt,
                                        "tool-call budget exhausted",
                                        &audit,
                                    )
                                    .await;
                                blocked.state = VerificationState::Blocked;
                                blocked.verified = false;
                                blocked.summary = format!(
                                    "agent tool-call limit ({}) reached without verified success",
                                    config.max_tool_calls
                                );
                                return Ok(LoopOutcome {
                                    final_answer: format!("Blocked: {}", blocked.summary),
                                    verification: blocked,
                                });
                            }
                            let started=AgentEvent::ToolStartedWithId { tool: call.name.clone(), call_id: call.call_id.clone() }; if let Some(tx)=&progress{let _=tx.send(started.clone());}
                            // Keep the historical, correlation-free event in the
                            // durable answer for API compatibility. The live
                            // transport receives only the identity-bearing event
                            // so a late completion cannot close another tool.
                            provider_events.push(AgentEvent::ToolStarted(call.name.clone()));
                            provider_events.push(started);
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
                                        output: bound_text(redact_text(&format!("tool was not executed because its audit record could not be created: {error}")), config.tool_output_max_chars),
                                        is_error: true,
                                    });
                                    continue;
                                }
                            };
                            if call.name == "termux_job" && !config.execution_plan_enabled {
                                let msg = "termux_job is disabled by configuration (execution_plan_enabled = false)";
                                self.storage.set_tool_run_status(&tool_run_id, "failed", None, Some(msg))?;
                                next.push(ToolResult {
                                    call_id: call.call_id.clone(),
                                    name: call.name.clone(),
                                    output: msg.into(),
                                    is_error: true,
                                });
                                continue;
                            }
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
                                let completed = AgentEvent::ToolCompletedWithId {
                                    tool: call.name.clone(),
                                    call_id: call.call_id.clone(),
                                    summary: message.into(),
                                };
                                if let Some(tx) = &progress { let _ = tx.send(completed.clone()); }
                                provider_events.push(AgentEvent::ToolCompleted {
                                    tool: call.name.clone(),
                                    summary: message.into(),
                                });
                                provider_events.push(completed);
                                next.push(result);
                                if identical_failure_repeats
                                    >= config.max_no_progress_repeats
                                {
                                    let audit = self.storage.tool_runs(principal, &agent_run_id)?;
                                    let mut blocked = completion
                                        .verify_for_task_async(prompt, "no progress", &audit)
                                        .await;
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
                            let tool_remaining = std::time::Duration::from_secs(config.max_runtime_seconds)
                                .saturating_sub(run_started.elapsed());
                            let mut execution=tokio::select!{
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
                            if execution.status == crate::tools::ToolRunStatus::AwaitingApproval {
                                self.storage.set_tool_run_status(
                                    &tool_run_id,
                                    "awaiting_approval",
                                    None,
                                    Some(&execution.result.output),
                                )?;
                                self.storage.set_agent_run_status(
                                    principal,
                                    &agent_run_id,
                                    "awaiting_approval",
                                    None,
                                )?;
                                if let Some(approval_id) = execution.approval_id.clone() {
                                    let requested = AgentEvent::ApprovalRequested {
                                        approval_id,
                                        tool: call.name.clone(),
                                        call_id: call.call_id.clone(),
                                        summary: bound_text(
                                            redact_text(&execution.result.output),
                                            1_024,
                                        ),
                                    };
                                    if let Some(tx) = &progress {
                                        let _ = tx.send(requested.clone());
                                    }
                                    provider_events.push(requested);
                                }
                                let approval_status = AgentEvent::Status(format!(
                                    "Owner approval required before continuing: {}",
                                    execution.result.output
                                ));
                                provider_events.push(approval_status);
                                let wait_for = tool_remaining.min(std::time::Duration::from_secs(15 * 60));
                                match self.tools.wait_for_exact_approval(
                                    &call,
                                    &tool_context,
                                    wait_for,
                                    token.clone(),
                                ).await? {
                                    crate::tools::ApprovalWaitStatus::Approved => {
                                        self.storage.set_agent_run_status(principal, &agent_run_id, "running", None)?;
                                        self.storage.set_tool_run_status(&tool_run_id, "running", None, None)?;
                                        execution = self.tools.execute(&call, &tool_context).await;
                                    }
                                    crate::tools::ApprovalWaitStatus::Denied => {
                                        self.storage.set_agent_run_status(principal, &agent_run_id, "running", None)?;
                                        execution.status = crate::tools::ToolRunStatus::Denied;
                                        execution.result.output = "owner denied this exact operation".into();
                                    }
                                    crate::tools::ApprovalWaitStatus::Expired => {
                                        execution.result.output = "exact approval expired before it could be consumed".into();
                                    }
                                    crate::tools::ApprovalWaitStatus::TimedOut => {
                                        execution.result.output = "approval wait reached the bounded runtime limit".into();
                                    }
                                    crate::tools::ApprovalWaitStatus::Cancelled => {
                                        return Err(anyhow!("generation cancelled while awaiting approval"));
                                    }
                                }
                            }
                            if execution.approval_mode.is_some()
                                || execution.policy_original.is_some()
                            {
                                self.storage.set_tool_run_approval_audit(
                                    &tool_run_id,
                                    execution.approval_mode.as_deref(),
                                    execution.policy_original.as_deref(),
                                )?;
                            }
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
                            if result.is_error {
                                failed_actions.insert(action_signature);
                            } else {
                                failed_actions.clear();
                                identical_failure_repeats = 0;
                                for artifact in artifacts_from_tool_output(&result.output) {
                                    artifacts.insert(artifact.path.clone(), artifact);
                                }
                            }
                            let summary=if result.is_error { format!("failed: {}",result.output) } else { "completed".into() };
                            let completed=AgentEvent::ToolCompletedWithId{tool:call.name.clone(),call_id:call.call_id.clone(),summary:summary.clone()}; if let Some(tx)=&progress{let _=tx.send(completed.clone());}
                            provider_events.push(AgentEvent::ToolCompleted { tool: call.name.clone(), summary });
                            provider_events.push(completed);
                            next.push(result);
                        }
                        let signature = next.iter().map(|result| format!("{}:{}:{}", result.name, result.is_error, short_hash(&result.output))).collect::<Vec<_>>().join("|");
                        observation_signatures.push_back(signature);
                        let max_signatures = (config.max_no_progress_repeats.max(2) * 2).max(6);
                        while observation_signatures.len() > max_signatures {
                            observation_signatures.pop_front();
                        }
                        if result_aware_ping_pong(&observation_signatures, config.max_no_progress_repeats) {
                            let audit=self.storage.tool_runs(principal,&agent_run_id)?;
                            let mut blocked=completion.verify_for_task_async(prompt,"repeated equivalent observations",&audit).await;
                            blocked.state=VerificationState::Blocked; blocked.verified=false;
                            blocked.summary="bounded no-progress limit reached after result-aware ping-pong".into();
                            return Ok(LoopOutcome { final_answer:format!("Blocked: {}",blocked.summary), verification:blocked });
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
            self.storage.record_agent_run_event(&agent_run_id, "final_answer_ready", timing_started.elapsed().as_millis() as u64, &serde_json::json!({}))?;
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
                let meaningful = verification.task_kind.is_action_like();
                let reusable = meaningful;
                let dependency_installs = self
                    .storage
                    .dependency_installs(&agent_run_id)
                    .unwrap_or_default();
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
                            operation: bound_text(tool.arguments_json.clone(), 1_000),
                            observable_summary: bound_text(
                                redact_text(
                                    tool.output
                                        .as_deref()
                                        .or(tool.error.as_deref())
                                        .unwrap_or("no output"),
                                ),
                                500,
                            ),
                            verification: tool
                                .output
                                .as_deref()
                                .is_some_and(|output| {
                                    output.contains("\"verification_evidence\":true")
                                        || output.contains("\"verified\":true")
                                        || output.contains("\"exists\":true")
                                }),
                        })
                        .collect(),
                    installed_dependencies: dependency_installs
                        .into_iter()
                        .filter(|install| install.status == "succeeded")
                        .map(|install| format!("{} ({})", install.binary, install.package))
                        .collect(),
                    artifacts: artifacts
                        .values()
                        .map(|artifact| artifact.path.display().to_string())
                        .collect(),
                    final_observable_result: bound_text(
                        redact_text(&final_answer),
                        2_000,
                    ),
                    verification_evidence: verification.summary.clone(),
                    skill_candidate: None,
                };
                // Persist before returning, but keep the job unreleased until
                // the frontend records final delivery acknowledgement.
                if config.background_learning {
                    self.storage.enqueue_learning_payload(principal,&agent_run_id,&serde_json::json!({"trace":trace,"explicit_prompt":append_user.then_some(bound_text(redact_text(prompt),2_000))}))?;
                }
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
                run_id: agent_run_id.clone(),
                progress: events,
                final_answer,
                side_mode: ctx.mode == ChatMode::Side,
                artifacts: artifacts.into_values().collect(),
            })
        }.await;

        self.active.lock().unwrap().remove(&active_key);
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

fn run_key(
    principal: &str,
    scope: Option<TelegramScope>,
    explicit_session: Option<&str>,
) -> String {
    if let Some(session_id) = explicit_session {
        return format!("{principal}:cli-session:{session_id}");
    }
    scope
        .map(|scope| {
            format!(
                "{principal}:telegram:{}:{}",
                scope.chat_id,
                scope.thread_key()
            )
        })
        .unwrap_or_else(|| format!("{principal}:cli-active"))
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

fn short_hash(value: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(redact_text(value).as_bytes()))[..16].to_owned()
}

fn result_aware_ping_pong(signatures: &std::collections::VecDeque<String>, repeats: usize) -> bool {
    let repeats = repeats.max(2);
    if signatures.len() < repeats * 2 {
        return false;
    }
    let values = signatures
        .iter()
        .rev()
        .take(repeats * 2)
        .collect::<Vec<_>>();
    values.windows(3).all(|window| window[0] == window[2])
}

fn compact_provider_messages(
    messages: &mut Vec<crate::storage::MessageRecord>,
    max_chars: usize,
    threshold: usize,
) {
    let total = messages
        .iter()
        .map(|message| message.content.chars().count())
        .sum::<usize>();
    if total <= threshold.min(max_chars) {
        return;
    }
    let mut kept = Vec::new();
    let mut used = 0usize;
    for message in messages.iter().rev() {
        let size = message.content.chars().count();
        if used + size > max_chars.saturating_sub(512) {
            continue;
        }
        kept.push(message.clone());
        used += size;
    }
    kept.reverse();
    kept.insert(0, crate::storage::MessageRecord { role:"system".into(), content:"Earlier provider context was compacted. Durable messages and raw tool audit remain stored; rely only on the bounded observable context below.".into(), created_at:chrono::Utc::now().to_rfc3339() });
    *messages = kept;
}

#[allow(clippy::too_many_arguments)]
fn run_observations_block<'a>(
    goal: &str,
    verification: &CompletionEvidence,
    tool_runs: &[crate::storage::ToolRunRecord],
    installs: &[crate::storage::DependencyInstallRecord],
    artifacts: impl Iterator<Item = &'a AgentArtifact>,
    attempt_count: usize,
    remaining_turns: usize,
    remaining_tool_calls: usize,
    remaining_runtime_seconds: u64,
) -> String {
    let summarize = |run: &crate::storage::ToolRunRecord| {
        serde_json::json!({
            "tool": bound_text(redact_text(&run.tool_name), 128),
            "operation": bounded_json(
                &serde_json::from_str(&run.arguments_json).unwrap_or_else(|_| serde_json::json!({"summary":run.arguments_json})),
                1_500,
            ),
            "status": run.status,
            "observation": bound_text(
                redact_text(run.output.as_deref().or(run.error.as_deref()).unwrap_or("no bounded output")),
                1_000,
            ),
        })
    };
    let successful = tool_runs
        .iter()
        .filter(|run| run.status == "succeeded")
        .take(32)
        .map(summarize)
        .collect::<Vec<_>>();
    let failed = tool_runs
        .iter()
        .filter(|run| matches!(run.status.as_str(), "failed" | "denied" | "interrupted"))
        .take(24)
        .map(summarize)
        .collect::<Vec<_>>();
    let installed = installs
        .iter()
        .filter(|install| install.status == "succeeded")
        .take(24)
        .map(|install| {
            serde_json::json!({
                "binary":install.binary,
                "package":install.package,
                "evidence":install.evidence,
            })
        })
        .collect::<Vec<_>>();
    let artifacts = artifacts
        .take(32)
        .map(|artifact| {
            serde_json::json!({
                "path":artifact.path,
                "name":artifact.name,
                "size_bytes":artifact.size_bytes,
            })
        })
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "goal":bound_text(redact_text(goal), 2_000),
        "task_kind":verification.task_kind,
        "successful_actions":successful,
        "failed_actions":failed,
        "installed_dependencies":installed,
        "current_artifacts_state":artifacts,
        "verification_evidence":verification.observable_evidence,
        "missing_evidence":verification.summary,
        "attempt_count":attempt_count,
        "remaining_budgets":{
            "turns":remaining_turns,
            "tool_calls":remaining_tool_calls,
            "runtime_seconds":remaining_runtime_seconds,
        },
        "runtime_instruction":"Continue from these observable facts. Add new evidence or choose a materially different action after failure. Do not merely claim completion."
    });
    format!(
        "<RUN_OBSERVATIONS>{}</RUN_OBSERVATIONS>",
        bounded_json(&payload, 24_000)
    )
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
        attachments::{AttachmentIngest, AttachmentKind, ScannedPdfProcessor},
        auth::AuthManager,
        config::AttachmentConfig,
        providers::{Provider, ProviderCapabilities, ProviderResponse, ProviderStep, ProviderTurn},
        tools::{Tool, ToolCall, ToolRisk, ToolSpec},
    };
    use async_trait::async_trait;
    use std::path::Path;
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

    struct NoLocalOcr;

    impl ScannedPdfProcessor for NoLocalOcr {
        fn extract(
            &self,
            _pdf: &[u8],
            _scratch_root: &Path,
            _config: &AttachmentConfig,
        ) -> Result<Option<Vec<crate::attachments::ScannedPdfPage>>> {
            Ok(None)
        }
    }

    struct VisionPhotoProvider {
        calls: AtomicUsize,
        expected_bytes: Vec<u8>,
    }

    #[async_trait]
    impl Provider for VisionPhotoProvider {
        fn id(&self) -> &'static str {
            "vision-photo"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities {
                text: true,
                vision: true,
                file_input: false,
                native_tools: true,
                tool_protocol: ToolProtocol::Native,
                model_discovery: false,
                structured_output: true,
                continuation: false,
                evidence: "deterministic vision photo provider fixture".into(),
            }
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
            tool_results: Vec<ToolResult>,
            progress: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderTurn> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.images.len(), 1);
            assert_eq!(request.images[0].bytes, self.expected_bytes);
            assert!(continuation.is_none());
            assert!(tool_results.is_empty());
            if let Some(tx) = progress {
                let _ = tx.send(AgentEvent::TextDelta("Ini adalah gambar merah.".into()));
            }
            Ok(ProviderTurn {
                step: ProviderStep::Final("Ini adalah gambar merah.".into()),
                continuation: None,
                events: vec![],
            })
        }
    }

    fn sample_png() -> Vec<u8> {
        let img: image::ImageBuffer<image::Rgb<u8>, Vec<u8>> =
            image::ImageBuffer::from_pixel(2, 2, image::Rgb([255, 0, 0]));
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .unwrap();
        buf
    }
    struct AgentPdfProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Provider for AgentPdfProvider {
        fn id(&self) -> &'static str {
            "pdf-agent"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities {
                text: true,
                vision: false,
                file_input: true,
                native_tools: false,
                tool_protocol: ToolProtocol::ChatOnly,
                model_discovery: false,
                structured_output: false,
                continuation: false,
                evidence: "deterministic provider file-input fixture".into(),
            }
        }
        async fn run(
            &self,
            request: ProviderRequest,
            _progress: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                assert_eq!(request.files.len(), 1);
                assert_eq!(request.files[0].mime_type, "application/pdf");
                assert!(request.files[0].bytes.starts_with(b"%PDF-"));
                return Ok(ProviderResponse {
                    events: vec![],
                    final_answer: "provider extracted the scanned PDF text".into(),
                });
            }
            assert!(request.files.is_empty());
            assert!(request.messages.iter().any(|message| {
                message.content.contains("SESSION_ATTACHMENTS")
                    && message
                        .content
                        .contains("provider extracted the scanned PDF text")
            }));
            Ok(ProviderResponse {
                events: vec![],
                final_answer: "The scanned PDF says: provider extracted the scanned PDF text"
                    .into(),
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
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities::native("test native protocol")
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

    struct ChatOnlyProbeProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Provider for ChatOnlyProbeProvider {
        fn id(&self) -> &'static str {
            "chat-only"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities::chat_only("deterministic fixture has no agent protocol")
        }
        async fn run(
            &self,
            _: ProviderRequest,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ProviderResponse {
                events: Vec::new(),
                final_answer: "informational response".into(),
            })
        }
    }

    struct RepeatedFailureProvider {
        turns: AtomicUsize,
    }

    #[async_trait]
    impl Provider for RepeatedFailureProvider {
        fn id(&self) -> &'static str {
            "repeat-failure"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities::native("deterministic native fixture")
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
                    call_id: format!("repeat-{turn}"),
                    name: "adaptive_action".into(),
                    arguments: serde_json::json!({"strategy":"bad"}),
                }]),
                continuation: None,
                events: Vec::new(),
            })
        }
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
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities::native("test native protocol")
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
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities::native("test native protocol")
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

    struct ParityProvider {
        id: &'static str,
        protocol: ToolProtocol,
        turns: AtomicUsize,
    }

    #[async_trait]
    impl Provider for ParityProvider {
        fn id(&self) -> &'static str {
            self.id
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities {
                text: true,
                vision: false,
                file_input: false,
                native_tools: self.protocol == ToolProtocol::Native,
                tool_protocol: self.protocol,
                model_discovery: self.id.starts_with("custom"),
                structured_output: true,
                continuation: true,
                evidence: format!("deterministic {} parity adapter", self.id),
            }
        }
        async fn run(
            &self,
            _: ProviderRequest,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderResponse> {
            Err(anyhow!("agent parity test requires normalized turns"))
        }
        async fn run_turn(
            &self,
            request: ProviderRequest,
            _: Option<serde_json::Value>,
            results: Vec<ToolResult>,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderTurn> {
            assert!(request
                .tools
                .iter()
                .any(|tool| tool.name == "parity_action"));
            assert!(request
                .tools
                .iter()
                .any(|tool| tool.name == "parity_verify"));
            let turn = self.turns.fetch_add(1, Ordering::SeqCst);
            let call = |call_id: &str, name: &str| ProviderTurn {
                step: ProviderStep::ToolCalls(vec![ToolCall {
                    call_id: call_id.into(),
                    name: name.into(),
                    arguments: serde_json::json!({}),
                }]),
                continuation: Some(serde_json::json!({ "turn": turn })),
                events: Vec::new(),
            };
            Ok(match turn {
                0 => {
                    assert!(results.is_empty());
                    call("action", "parity_action")
                }
                1 => {
                    assert_eq!(results.len(), 1);
                    assert!(!results[0].is_error);
                    call("verify", "parity_verify")
                }
                _ => {
                    assert_eq!(results.len(), 1);
                    assert!(results[0].output.contains("verification_evidence"));
                    ProviderTurn {
                        step: ProviderStep::Final("verified parity workflow".into()),
                        continuation: None,
                        events: Vec::new(),
                    }
                }
            })
        }
    }

    struct ParityTool {
        name: &'static str,
        risk: ToolRisk,
        output: &'static str,
    }

    #[async_trait]
    impl Tool for ParityTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.name.into(),
                description: "Execute one deterministic provider-parity observation".into(),
                parameters: serde_json::json!({
                    "type":"object","properties":{},"additionalProperties":false
                }),
                risk: self.risk,
                origin: crate::tools::ToolOrigin::Builtin,
                effect: crate::tools::ToolEffect::Idempotent,
                required_capabilities: Vec::new(),
                timeout_ms: 5_000,
            }
        }

        async fn execute(&self, _: &ToolContext, _: serde_json::Value) -> Result<String> {
            Ok(self.output.into())
        }
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
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities::native("test native protocol")
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
                    arguments: serde_json::json!({ "strategy": strategy }),
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
                    assert!(request.messages.iter().any(|message| {
                        message.content.contains("RUN_OBSERVATIONS")
                            && message.content.contains("missing_evidence")
                            && message.content.contains("remaining_budgets")
                    }));
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
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities::native("test native protocol")
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
        _provider_id: &str,
        provider: Arc<dyn Provider>,
    ) -> (Arc<AgentEngine>, Arc<Storage>, String, tempfile::TempDir) {
        let db = Arc::new(Storage::open_memory().unwrap());
        let sessions = Arc::new(SessionManager::new(db.clone()));
        let main = sessions.ensure_default_session("u").unwrap();
        // Normal v0.3 runtime admits only the Custom provider key. Individual
        // fake provider ids still exercise their protocol behavior behind that
        // key; legacy-session tests opt into a legacy key explicitly.
        db.set_session_provider("u", &main.id, "custom", None, "m")
            .unwrap();
        sessions.switch_main("u", &main.id).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let auth = Arc::new(AuthManager::new(db.clone(), tmp.path().join("secrets")));
        let providers = Arc::new(ProviderRegistry::from_single("custom", provider, auth));
        (
            Arc::new(AgentEngine::new(sessions, db.clone(), providers)),
            db,
            main.id,
            tmp,
        )
    }

    fn empty_pdf() -> Vec<u8> {
        let stream = "BT /F1 14 Tf 72 760 Td () Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        ];
        let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        pdf.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }

    #[tokio::test]
    async fn agent_engine_runs_scanned_pdf_provider_file_fallback_before_final_answer() {
        let provider = Arc::new(AgentPdfProvider {
            calls: AtomicUsize::new(0),
        });
        let (base, db, session, _fixture) = engine("pdf-agent", provider.clone());
        let attachment_root = tempfile::tempdir().unwrap();
        let attachments = Arc::new(
            AttachmentManager::new(
                db.clone(),
                attachment_root.path(),
                AttachmentConfig::default(),
            )
            .unwrap()
            .with_scanned_pdf_processor(Arc::new(NoLocalOcr)),
        );
        let record = attachments
            .ingest(AttachmentIngest {
                owner_id: "u".into(),
                session_id: session.clone(),
                telegram_file_id: None,
                telegram_unique_id: Some("agent-pdf-1".into()),
                original_name: "scan.pdf".into(),
                declared_mime: Some("application/pdf".into()),
                expected_kind: AttachmentKind::Document,
                bytes: empty_pdf(),
            })
            .unwrap();
        assert_eq!(record.processing_status, "needs_ocr");

        let agent = AgentEngine::with_registry_runtime(
            base.sessions.clone(),
            db.clone(),
            base.providers.clone(),
            AgentConfig::default(),
            base.tools.clone(),
            None,
            Some(attachments.clone()),
        );
        let answer = agent
            .submit_with_progress("u", "What does the attached document say?", None)
            .await
            .unwrap();
        assert!(answer.final_answer.contains("provider extracted"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        let stored = db.attachment("u", &record.attachment_id).unwrap().unwrap();
        assert_eq!(stored.processing_status, "ready");
        assert!(db
            .search_attachment_chunks("u", &session, "provider extracted", 5)
            .unwrap()
            .iter()
            .any(|chunk| chunk.text.contains("provider extracted")));
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
    async fn legacy_provider_history_is_read_only_until_custom_is_selected() {
        let (engine, db, session, _tmp) = engine("codex", Arc::new(EchoProvider));
        db.set_session_provider("u", &session, "codex", None, "m")
            .unwrap();
        db.append_message("u", &session, "assistant", "legacy answer")
            .unwrap();

        let error = engine
            .submit_with_progress("u", "new request", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("provider_configuration_required"));
        let messages = db.messages("u", &session).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "legacy answer");
        assert!(db.agent_runs("u", 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn chat_only_model_rejects_action_explicitly_but_serves_information() {
        let provider = Arc::new(ChatOnlyProbeProvider {
            calls: AtomicUsize::new(0),
        });
        let (engine, db, _, _tmp) = engine("chat-only", provider.clone());
        let error = engine
            .submit_with_progress("u", "Create the requested artifact", None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("explicitly ChatOnly"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(db.agent_runs("u", 10).unwrap()[0].status, "failed");

        let answer = engine
            .submit_with_progress("u", "What is an artifact?", None)
            .await
            .unwrap();
        assert_eq!(answer.final_answer, "informational response");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
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
        assert!(
            learned.is_empty(),
            "post-delivery learning must not block AgentAnswer"
        );
    }

    #[tokio::test]
    async fn successful_intermediate_action_resets_failed_actions_and_recovers() {
        struct RepairThenVerifyProvider {
            turns: AtomicUsize,
        }

        #[async_trait]
        impl Provider for RepairThenVerifyProvider {
            fn id(&self) -> &'static str {
                "repair-verify"
            }
            fn models(&self) -> Vec<String> {
                vec!["m".into()]
            }
            fn ready(&self) -> bool {
                true
            }
            fn capabilities(&self, _model: &str) -> ProviderCapabilities {
                ProviderCapabilities::native("deterministic native fixture")
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
                match turn {
                    0 => Ok(ProviderTurn {
                        step: ProviderStep::ToolCalls(vec![ToolCall {
                            call_id: "call-0".into(),
                            name: "adaptive_action".into(),
                            arguments: serde_json::json!({"strategy":"bad"}),
                        }]),
                        continuation: None,
                        events: Vec::new(),
                    }),
                    1 => Ok(ProviderTurn {
                        step: ProviderStep::ToolCalls(vec![ToolCall {
                            call_id: "call-1".into(),
                            name: "adaptive_action".into(),
                            arguments: serde_json::json!({"strategy":"repair"}),
                        }]),
                        continuation: None,
                        events: Vec::new(),
                    }),
                    2 => Ok(ProviderTurn {
                        step: ProviderStep::ToolCalls(vec![ToolCall {
                            call_id: "call-2".into(),
                            name: "adaptive_action".into(),
                            arguments: serde_json::json!({"strategy":"verify"}),
                        }]),
                        continuation: None,
                        events: Vec::new(),
                    }),
                    _ => Ok(ProviderTurn {
                        step: ProviderStep::Final("Repaired and verified".into()),
                        continuation: None,
                        events: Vec::new(),
                    }),
                }
            }
        }

        let provider = Arc::new(RepairThenVerifyProvider {
            turns: AtomicUsize::new(0),
        });
        let (base, db, _, _tmp) = engine("repair-verify", provider.clone());
        let registry = Arc::new(ToolRegistry::new(
            ToolPolicy::default().allow_side_effect("adaptive_action"),
            4_096,
        ));
        registry.register(AdaptiveActionTool).unwrap();
        let engine = AgentEngine::with_registry(
            base.sessions.clone(),
            db.clone(),
            base.providers.clone(),
            AgentConfig {
                max_turns: 6,
                max_no_progress_repeats: 2,
                ..AgentConfig::default()
            },
            registry,
        );
        let answer = engine
            .submit_with_progress("u", "Repair and verify the pipeline", None)
            .await
            .unwrap();
        assert_eq!(answer.final_answer, "Repaired and verified");
        let run = &db.agent_runs("u", 1).unwrap()[0];
        assert_eq!(run.status, "completed");
        let audit = db.tool_runs("u", &run.id).unwrap();
        assert_eq!(audit.len(), 3);
        assert_eq!(audit[0].status, "failed");
        assert_eq!(audit[1].status, "completed");
        assert_eq!(audit[2].status, "completed");
    }

    #[tokio::test]
    async fn repeated_identical_failed_action_terminates_as_bounded_blocker() {
        let provider = Arc::new(RepeatedFailureProvider {
            turns: AtomicUsize::new(0),
        });
        let (base, db, _, _tmp) = engine("repeat-failure", provider.clone());
        let registry = Arc::new(ToolRegistry::new(
            ToolPolicy::default().allow_side_effect("adaptive_action"),
            4_096,
        ));
        registry.register(AdaptiveActionTool).unwrap();
        let engine = AgentEngine::with_registry(
            base.sessions.clone(),
            db.clone(),
            base.providers.clone(),
            AgentConfig {
                max_turns: 6,
                max_no_progress_repeats: 2,
                ..AgentConfig::default()
            },
            registry,
        );
        let answer = engine
            .submit_with_progress("u", "Fix the widget workflow", None)
            .await
            .unwrap();
        assert!(answer.final_answer.contains("Blocked"));
        assert!(answer.final_answer.contains("no-progress"));
        assert_eq!(provider.turns.load(Ordering::SeqCst), 3);
        let run = &db.agent_runs("u", 1).unwrap()[0];
        assert_eq!(run.status, "blocked");
        let audit = db.tool_runs("u", &run.id).unwrap();
        assert_eq!(audit.len(), 3);
        assert_eq!(audit[0].status, "failed");
        assert!(audit[1..].iter().all(|tool| tool.status == "denied"));
    }

    #[tokio::test]
    async fn codex_antigravity_and_custom_protocols_keep_the_same_agent_tool_workflow() {
        for (id, protocol) in [
            ("codex-parity", ToolProtocol::Native),
            ("antigravity-parity", ToolProtocol::Native),
            ("custom-native-parity", ToolProtocol::Native),
            (
                "custom-structured-parity",
                ToolProtocol::StructuredJsonFallback,
            ),
        ] {
            let provider = Arc::new(ParityProvider {
                id,
                protocol,
                turns: AtomicUsize::new(0),
            });
            let (base, db, _, _tmp) = engine(id, provider.clone());
            let registry = Arc::new(ToolRegistry::new(
                ToolPolicy::default().allow_side_effect("parity_action"),
                4_096,
            ));
            registry
                .register(ParityTool {
                    name: "parity_action",
                    risk: ToolRisk::SideEffect,
                    output: "{\"artifact\":\"created\"}",
                })
                .unwrap();
            registry
                .register(ParityTool {
                    name: "parity_verify",
                    risk: ToolRisk::ReadOnly,
                    output: "{\"verification_evidence\":true}",
                })
                .unwrap();
            let engine = AgentEngine::with_registry(
                base.sessions.clone(),
                db.clone(),
                base.providers.clone(),
                AgentConfig::default(),
                registry,
            );
            let answer = engine
                .submit_with_progress("u", "Create the parity artifact", None)
                .await
                .unwrap();
            assert_eq!(answer.final_answer, "verified parity workflow", "{id}");
            assert_eq!(provider.turns.load(Ordering::SeqCst), 3, "{id}");
            let run = &db.agent_runs("u", 1).unwrap()[0];
            assert_eq!(run.status, "completed", "{id}");
            assert_eq!(db.tool_runs("u", &run.id).unwrap().len(), 2, "{id}");
        }
    }

    #[tokio::test]
    async fn yolo_auto_approval_is_persisted_on_the_durable_tool_run() {
        let provider = Arc::new(ParityProvider {
            id: "yolo-parity",
            protocol: ToolProtocol::Native,
            turns: AtomicUsize::new(0),
        });
        let (base, db, session, _tmp) = engine("yolo-parity", provider);
        db.set_session_yolo("u", &session, true).unwrap();
        let registry = Arc::new(ToolRegistry::new(ToolPolicy::default(), 4_096));
        registry
            .register(ParityTool {
                name: "parity_action",
                risk: ToolRisk::Privileged,
                output: "{\"changed\":true}",
            })
            .unwrap();
        registry
            .register(ParityTool {
                name: "parity_verify",
                risk: ToolRisk::ReadOnly,
                output: "{\"verification_evidence\":true}",
            })
            .unwrap();
        let engine = AgentEngine::with_registry(
            base.sessions.clone(),
            db.clone(),
            base.providers.clone(),
            AgentConfig::default(),
            registry,
        );
        engine
            .submit_with_progress("u", "Restart the typed service", None)
            .await
            .unwrap();
        let run = &db.agent_runs("u", 1).unwrap()[0];
        let tools = db.tool_runs("u", &run.id).unwrap();
        assert_eq!(
            tools[0].approval_mode.as_deref(),
            Some("yolo_auto_approved")
        );
        assert_eq!(tools[0].policy_original.as_deref(), Some("ask"));
        assert_eq!(tools[0].status, "succeeded");
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
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("provider did not start through the active Custom runtime key");
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

    #[tokio::test]
    async fn agent_engine_delivers_vision_photo_final_answer_without_blocked_no_progress() {
        let png_bytes = sample_png();
        let provider = Arc::new(VisionPhotoProvider {
            calls: AtomicUsize::new(0),
            expected_bytes: png_bytes.clone(),
        });
        let (base, db, session, _fixture) = engine("vision-photo", provider.clone());
        let attachment_root = tempfile::tempdir().unwrap();
        let attachments = Arc::new(
            AttachmentManager::new(
                db.clone(),
                attachment_root.path(),
                AttachmentConfig::default(),
            )
            .unwrap(),
        );
        let record = attachments
            .ingest(AttachmentIngest {
                owner_id: "u".into(),
                session_id: session.clone(),
                telegram_file_id: None,
                telegram_unique_id: Some("photo-unique-1".into()),
                original_name: "photo-1.jpg".into(),
                declared_mime: Some("image/png".into()),
                expected_kind: AttachmentKind::Image,
                bytes: png_bytes,
            })
            .unwrap();
        assert_eq!(record.processing_status, "ready");

        let agent = AgentEngine::with_registry_runtime(
            base.sessions.clone(),
            db.clone(),
            base.providers.clone(),
            AgentConfig::default(),
            base.tools.clone(),
            None,
            Some(attachments.clone()),
        );

        let prompt = format!(
            "Attachment received: {} (id={}, type={}, status={}). Apa ini",
            record.original_name,
            record.attachment_id,
            record.detected_mime,
            record.processing_status
        );
        let answer = agent
            .submit_with_progress("u", &prompt, None)
            .await
            .unwrap();

        assert_eq!(answer.final_answer, "Ini adalah gambar merah.");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        let runs = db.agent_runs("u", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "completed");
        let messages = db.messages("u", &session).unwrap();
        assert_eq!(messages.last().unwrap().content, "Ini adalah gambar merah.");
    }

    #[tokio::test]
    async fn agent_engine_truthfully_rejects_vision_when_provider_lacks_vision_capability() {
        let provider = Arc::new(ChatOnlyProbeProvider {
            calls: AtomicUsize::new(0),
        });
        let (base, db, session, _fixture) = engine("chat-only", provider.clone());
        let attachment_root = tempfile::tempdir().unwrap();
        let attachments = Arc::new(
            AttachmentManager::new(
                db.clone(),
                attachment_root.path(),
                AttachmentConfig::default(),
            )
            .unwrap(),
        );
        let record = attachments
            .ingest(AttachmentIngest {
                owner_id: "u".into(),
                session_id: session.clone(),
                telegram_file_id: None,
                telegram_unique_id: Some("photo-unique-2".into()),
                original_name: "photo-2.jpg".into(),
                declared_mime: Some("image/png".into()),
                expected_kind: AttachmentKind::Image,
                bytes: sample_png(),
            })
            .unwrap();
        assert_eq!(record.processing_status, "ready");

        let agent = AgentEngine::with_registry_runtime(
            base.sessions.clone(),
            db.clone(),
            base.providers.clone(),
            AgentConfig::default(),
            base.tools.clone(),
            None,
            Some(attachments.clone()),
        );

        let prompt = format!(
            "Attachment received: {} (id={}, type={}, status={}). Apa ini",
            record.original_name,
            record.attachment_id,
            record.detected_mime,
            record.processing_status
        );
        let error = agent
            .submit_with_progress("u", &prompt, None)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("does not declare vision capability"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    struct StreamingTestProvider {
        received_streaming: Arc<Mutex<Vec<bool>>>,
    }

    #[async_trait]
    impl Provider for StreamingTestProvider {
        fn id(&self) -> &'static str {
            "streaming-test"
        }
        fn models(&self) -> Vec<String> {
            vec!["m".into()]
        }
        fn ready(&self) -> bool {
            true
        }
        fn capabilities(&self, _model: &str) -> ProviderCapabilities {
            ProviderCapabilities {
                text: true,
                vision: false,
                file_input: false,
                native_tools: false,
                tool_protocol: ToolProtocol::ChatOnly,
                model_discovery: false,
                structured_output: true,
                continuation: false,
                evidence: "streaming test provider fixture".into(),
            }
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
            _: Vec<ToolResult>,
            progress: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderTurn> {
            self.received_streaming
                .lock()
                .unwrap()
                .push(request.streaming);
            if request.streaming {
                if let Some(tx) = progress {
                    let _ = tx.send(AgentEvent::TextDelta("streaming token".into()));
                }
            }
            Ok(ProviderTurn {
                step: ProviderStep::Final("deterministic final response".into()),
                continuation: None,
                events: vec![AgentEvent::Status("completed".into())],
            })
        }
    }

    #[tokio::test]
    async fn agent_engine_honors_provider_streaming_false_without_emitting_text_deltas() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(StreamingTestProvider {
            received_streaming: recorded.clone(),
        });
        let (base, db, session, _fixture) = engine("streaming-test", provider);
        let config = AgentConfig {
            provider_streaming: false,
            ..AgentConfig::default()
        };
        let agent = AgentEngine::with_registry_runtime(
            base.sessions.clone(),
            db.clone(),
            base.providers.clone(),
            config,
            base.tools.clone(),
            None,
            None,
        );

        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let answer = agent
            .submit_with_progress("u", "hello non-stream", Some(progress_tx))
            .await
            .unwrap();

        assert_eq!(answer.final_answer, "deterministic final response");
        assert_eq!(&*recorded.lock().unwrap(), &[false]);

        let mut events = Vec::new();
        while let Ok(event) = progress_rx.try_recv() {
            events.push(event);
        }
        assert!(!events.iter().any(|e| matches!(e, AgentEvent::TextDelta(_))));
        let messages = db.messages("u", &session).unwrap();
        assert_eq!(
            messages.last().unwrap().content,
            "deterministic final response"
        );
    }

    #[tokio::test]
    async fn agent_engine_honors_provider_streaming_true_and_preserves_streaming_deltas() {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(StreamingTestProvider {
            received_streaming: recorded.clone(),
        });
        let (base, db, session, _fixture) = engine("streaming-test", provider);
        let config = AgentConfig {
            provider_streaming: true,
            ..AgentConfig::default()
        };
        let agent = AgentEngine::with_registry_runtime(
            base.sessions.clone(),
            db.clone(),
            base.providers.clone(),
            config,
            base.tools.clone(),
            None,
            None,
        );

        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let answer = agent
            .submit_with_progress("u", "hello streaming", Some(progress_tx))
            .await
            .unwrap();

        assert_eq!(answer.final_answer, "deterministic final response");
        assert_eq!(&*recorded.lock().unwrap(), &[true]);

        let mut events = Vec::new();
        while let Ok(event) = progress_rx.try_recv() {
            events.push(event);
        }
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta(t) if t == "streaming token")));
        let messages = db.messages("u", &session).unwrap();
        assert_eq!(
            messages.last().unwrap().content,
            "deterministic final response"
        );
    }

    #[test]
    fn ping_pong_guard_detects_true_identical_hashes_and_allows_changing_repair_progress() {
        let mut progress_signatures = std::collections::VecDeque::new();
        // Turn 1: create artifact v1 (hash1)
        progress_signatures.push_back("termux_terminal:false:hash1".into());
        // Turn 2: inspect artifact v1 (error: invalid xref)
        progress_signatures.push_back("termux_terminal:true:invalid_xref".into());
        // Turn 3: repair artifact v2 (hash2 - DIFFERENT!)
        progress_signatures.push_back("termux_terminal:false:hash2".into());
        // Turn 4: inspect artifact v2 (valid - DIFFERENT!)
        progress_signatures.push_back("termux_terminal:false:valid_pdf".into());

        assert!(!result_aware_ping_pong(&progress_signatures, 2));

        // In contrast, true identical alternating results A-B-A-B:
        let mut loop_signatures = std::collections::VecDeque::new();
        loop_signatures.push_back("termux_terminal:false:hash1".into());
        loop_signatures.push_back("termux_terminal:true:invalid_xref".into());
        loop_signatures.push_back("termux_terminal:false:hash1".into());
        loop_signatures.push_back("termux_terminal:true:invalid_xref".into());

        assert!(result_aware_ping_pong(&loop_signatures, 2));
    }

    #[tokio::test]
    async fn informational_prompts_do_not_expose_execution_tools() {
        let provider = Arc::new(EchoProvider);
        let (engine, db, _session, _tmp) = engine("info-tool-filter", provider);
        let run = engine
            .submit_with_progress(
                "u",
                "How do I implement quicksort in Rust? Give me an example",
                None,
            )
            .await
            .unwrap();
        assert!(!run.final_answer.is_empty());
        let tools = db.tool_runs("u", &run.run_id).unwrap();
        // Informational question runs without exposed execution tools
        assert!(tools.iter().all(|t| t.risk == "read_only"));
    }
}
