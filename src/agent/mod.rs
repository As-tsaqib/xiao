use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Result};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    providers::{AgentEvent, ProviderRegistry, ProviderRequest, ProviderStep},
    session::{ChatMode, SessionManager},
    storage::Storage,
    tools::ToolRouter,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentAnswer {
    /// Safe status events only. No private reasoning or provider hidden chain-of-thought is represented here.
    pub progress: Vec<AgentEvent>,
    /// Persistent user-facing final answer only.
    pub final_answer: String,
    pub side_mode: bool,
}

pub struct AgentEngine {
    sessions: Arc<SessionManager>,
    storage: Arc<Storage>,
    providers: Arc<ProviderRegistry>,
    active: Mutex<HashMap<String, CancellationToken>>,
    tools: ToolRouter,
}

impl AgentEngine {
    pub fn new(
        sessions: Arc<SessionManager>,
        storage: Arc<Storage>,
        providers: Arc<ProviderRegistry>,
    ) -> Self {
        Self {
            sessions,
            storage,
            providers,
            active: Mutex::new(HashMap::new()),
            tools: ToolRouter,
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

        let result = async {
            let context = self.sessions.agent_context(principal)?;
            let provider = self.providers.get(&ctx.active.provider)?;
            let request = ProviderRequest {
                session_id: ctx.active.id.clone(),
                account_id: ctx.active.account_id.clone(),
                model: ctx.active.model.clone(),
                messages: context,
            };
            let started = AgentEvent::GenerationStarted;
            if let Some(tx) = &progress { let _ = tx.send(started.clone()); }
            let mut continuation=None;
            let mut tool_results=vec![];
            let mut provider_events=vec![];
            let final_answer = loop {
                let turn = tokio::select! {
                    _ = token.cancelled() => return Err(anyhow!("generation cancelled")),
                    response = provider.run_turn(request.clone(), continuation.take(), tool_results, progress.clone()) => response?,
                };
                provider_events.extend(turn.events);
                match turn.step {
                    ProviderStep::Final(answer)=>break answer,
                    ProviderStep::ToolCalls(calls)=>{
                        if calls.is_empty(){return Err(anyhow!("provider returned an empty tool-call turn"));}
                        continuation=turn.continuation;
                        let mut next=Vec::with_capacity(calls.len());
                        for call in calls {
                            let started=AgentEvent::ToolStarted(call.name.clone()); if let Some(tx)=&progress{let _=tx.send(started.clone());} provider_events.push(started);
                            let result=tokio::select!{ _=token.cancelled()=>return Err(anyhow!("generation cancelled")), result=self.tools.execute(&call,&request)=>result };
                            let summary=if result.is_error { format!("failed: {}",result.output) } else { "completed".into() };
                            let completed=AgentEvent::ToolCompleted{tool:call.name.clone(),summary}; if let Some(tx)=&progress{let _=tx.send(completed.clone());} provider_events.push(completed);
                            next.push(result);
                        }
                        tool_results=next;
                    }
                }
            };
            if token.is_cancelled() { return Err(anyhow!("generation cancelled")); }
            // Capture the active session before provider execution. A concurrent /session
            // command must never redirect this generation's final write into another session.
            self.sessions.append_assistant_to(principal, &ctx.active.id, &final_answer)?;
            if ctx.mode == ChatMode::Main && ctx.active.name.starts_with("Session ") && ctx.active.message_count <= 1 {
                let title = automatic_title(prompt);
                if !title.is_empty() { let _ = self.storage.rename_session(principal, &ctx.active.id, &title); }
            }
            let completed = AgentEvent::GenerationCompleted;
            if let Some(tx) = &progress { let _ = tx.send(completed.clone()); }
            let mut events = vec![started];
            events.extend(provider_events);
            events.push(completed);
            Ok(AgentAnswer { progress: events, final_answer, side_mode: ctx.mode == ChatMode::Side })
        }.await;

        self.active.lock().unwrap().remove(principal);
        if let Err(error) = &result {
            if let Some(tx) = &progress {
                let _ = tx.send(AgentEvent::GenerationFailed(error.to_string()));
            }
        }
        result
    }
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
        tools::ToolCall,
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
            tool_results: Vec<crate::tools::ToolResult>,
            _: Option<mpsc::UnboundedSender<AgentEvent>>,
        ) -> Result<ProviderTurn> {
            let turn = self.turns.fetch_add(1, Ordering::SeqCst);
            if turn == 0 {
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
