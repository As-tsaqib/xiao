use futures::{stream::FuturesUnordered, StreamExt};
use std::{future::Future, sync::Arc};
use tokio::sync::Semaphore;

use crate::tools::{ToolCall, ToolRisk, ToolSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionClass {
    ReadOnlyParallelSafe,
    Sequential,
}

pub fn execution_class(spec: Option<&ToolSpec>) -> ToolExecutionClass {
    match spec {
        Some(spec) if spec.risk == ToolRisk::ReadOnly => ToolExecutionClass::ReadOnlyParallelSafe,
        _ => ToolExecutionClass::Sequential,
    }
}

pub async fn schedule<T, F, Fut>(
    calls: Vec<ToolCall>,
    enabled: bool,
    limit: usize,
    classify: impl Fn(&ToolCall) -> ToolExecutionClass,
    execute: F,
) -> Vec<T>
where
    T: Send,
    F: Fn(ToolCall) -> Fut + Clone,
    Fut: Future<Output = T> + Send,
{
    let mut output = Vec::with_capacity(calls.len());
    let mut pending = calls.into_iter().peekable();
    while let Some(call) = pending.next() {
        if !enabled || classify(&call) == ToolExecutionClass::Sequential {
            output.push(execute.clone()(call).await);
            continue;
        }
        let mut group = vec![call];
        while pending
            .peek()
            .is_some_and(|next| classify(next) == ToolExecutionClass::ReadOnlyParallelSafe)
        {
            group.push(pending.next().expect("peeked call exists"));
        }
        let semaphore = Arc::new(Semaphore::new(limit.clamp(1, 16)));
        let mut tasks = FuturesUnordered::new();
        for (index, call) in group.into_iter().enumerate() {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore open");
            let execute = execute.clone();
            tasks.push(async move {
                let _permit = permit;
                (index, execute(call).await)
            });
        }
        let mut group_output = Vec::with_capacity(tasks.len());
        while let Some(value) = tasks.next().await {
            group_output.push(value);
        }
        group_output.sort_by_key(|(index, _)| *index);
        output.extend(group_output.into_iter().map(|(_, value)| value));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            call_id: id.into(),
            name: name.into(),
            arguments: json!({}),
        }
    }

    #[tokio::test]
    async fn parallel_reads_keep_order_and_mutations_are_barriers() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let sequence = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let results = schedule(
            vec![
                call("1", "read"),
                call("2", "read"),
                call("3", "write"),
                call("4", "read"),
            ],
            true,
            2,
            |call| {
                if call.name == "read" {
                    ToolExecutionClass::ReadOnlyParallelSafe
                } else {
                    ToolExecutionClass::Sequential
                }
            },
            {
                let active = active.clone();
                let peak = peak.clone();
                let sequence = sequence.clone();
                move |call| {
                    let active = active.clone();
                    let peak = peak.clone();
                    let sequence = sequence.clone();
                    async move {
                        let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        sequence
                            .lock()
                            .await
                            .push(format!("start:{}", call.call_id));
                        tokio::time::sleep(Duration::from_millis(if call.call_id == "1" {
                            30
                        } else {
                            5
                        }))
                        .await;
                        sequence.lock().await.push(format!("end:{}", call.call_id));
                        active.fetch_sub(1, Ordering::SeqCst);
                        call.call_id
                    }
                }
            },
        )
        .await;
        assert_eq!(results, ["1", "2", "3", "4"]);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        let sequence = sequence.lock().await;
        assert!(
            sequence.iter().position(|v| v == "end:2").unwrap()
                < sequence.iter().position(|v| v == "start:3").unwrap()
        );
        assert!(
            sequence.iter().position(|v| v == "end:3").unwrap()
                < sequence.iter().position(|v| v == "start:4").unwrap()
        );
    }
}
