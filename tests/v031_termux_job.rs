use serde_json::json;
use tokio_util::sync::CancellationToken;
use xiao::tools::{
    builtin::terminal::TermuxTerminalTool, builtin::TermuxJobTool, Tool, ToolContext,
};

#[test]
fn termux_job_schema_and_bounds() {
    let tool = TermuxJobTool::new(TermuxTerminalTool::new_unprivileged(), 32);
    let spec = tool.spec();
    assert_eq!(spec.name, "termux_job");
}

#[tokio::test]
async fn termux_job_rejects_empty_or_excessive_steps() {
    let tool = TermuxJobTool::new(TermuxTerminalTool::new_unprivileged(), 32);

    let ctx = ToolContext {
        principal: "owner-1".into(),
        session_id: "sess-1".into(),
        agent_run_id: "run-1".into(),
        yolo_mode: false,
        messages: vec![],
        cancellation: CancellationToken::new(),
        progress: None,
    };

    // Empty steps
    let res = tool.execute(&ctx, json!({ "steps": [] })).await;
    assert!(res.is_err());

    // Steps exceeding max (e.g. 33 steps)
    let steps: Vec<serde_json::Value> = (0..33)
        .map(|i| json!({ "id": format!("step-{i}"), "program": "echo", "args": ["hi"] }))
        .collect();
    let res = tool.execute(&ctx, json!({ "steps": steps })).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn termux_job_denies_root_escalation() {
    let tool = TermuxJobTool::new(TermuxTerminalTool::new_unprivileged(), 32);

    let ctx = ToolContext {
        principal: "owner-1".into(),
        session_id: "sess-1".into(),
        agent_run_id: "run-1".into(),
        yolo_mode: false,
        messages: vec![],
        cancellation: CancellationToken::new(),
        progress: None,
    };

    let res = tool
        .execute(
            &ctx,
            json!({
                "steps": [
                    { "id": "step-su", "program": "su", "args": ["-c", "id"] }
                ]
            }),
        )
        .await
        .unwrap();

    assert!(res.contains("denied"));
    assert!(res.contains("failed"));
}
