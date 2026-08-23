use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncReadExt, process::Command};
use tokio_util::sync::CancellationToken;

use crate::security::redact::redact_text;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AndroidOperation {
    InspectXiaoService,
    RestartXiaoService,
}

impl AndroidOperation {
    pub fn capability(&self) -> &'static str {
        match self {
            Self::InspectXiaoService => "android.service.inspect",
            Self::RestartXiaoService => "android.service.restart",
        }
    }

    pub fn requires_approval(&self) -> bool {
        matches!(self, Self::RestartXiaoService)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AndroidOutcome {
    pub operation: AndroidOperation,
    pub succeeded: bool,
    pub evidence: String,
    pub verified: bool,
}

#[async_trait]
pub trait AndroidBroker: Send + Sync {
    async fn execute(
        &self,
        operation: AndroidOperation,
        cancellation: CancellationToken,
    ) -> Result<AndroidOutcome>;
}

/// Minimal privileged broker for Xiao's own Android init service. It exposes
/// no arbitrary command or shell string. The daemon must itself run as UID 0;
/// a merely detected `su` binary is never invoked with model-controlled data.
#[derive(Debug, Clone)]
pub struct SystemAndroidBroker {
    getprop: PathBuf,
    setprop: PathBuf,
    service_name: String,
}

impl Default for SystemAndroidBroker {
    fn default() -> Self {
        Self {
            getprop: "/system/bin/getprop".into(),
            setprop: "/system/bin/setprop".into(),
            service_name: "xiao".into(),
        }
    }
}

impl SystemAndroidBroker {
    async fn status(&self, cancellation: &CancellationToken) -> Result<String> {
        let mut child = Command::new(&self.getprop)
            .arg(format!("init.svc.{}", self.service_name))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn typed Android getprop {}", self.getprop.display()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Android stdout pipe missing"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Android stderr pipe missing"))?;
        let status = tokio::select! {
            _ = cancellation.cancelled() => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(anyhow!("Android operation cancelled"));
            }
            result = child.wait() => result?,
        };
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        stdout.read_to_end(&mut stdout_bytes).await?;
        stderr.read_to_end(&mut stderr_bytes).await?;
        if !status.success() {
            return Err(anyhow!(
                "Android service inspection failed: {}",
                redact_text(&String::from_utf8_lossy(&stderr_bytes))
            ));
        }
        Ok(redact_text(String::from_utf8_lossy(&stdout_bytes).trim()))
    }
}

#[async_trait]
impl AndroidBroker for SystemAndroidBroker {
    async fn execute(
        &self,
        operation: AndroidOperation,
        cancellation: CancellationToken,
    ) -> Result<AndroidOutcome> {
        #[cfg(unix)]
        if unsafe { libc::geteuid() } != 0 {
            return Err(anyhow!(
                "typed Android broker requires xiaod to run as UID 0"
            ));
        }
        match operation {
            AndroidOperation::InspectXiaoService => {
                let state = self.status(&cancellation).await?;
                Ok(AndroidOutcome {
                    operation,
                    succeeded: !state.is_empty(),
                    evidence: format!("init.svc.{}={state}", self.service_name),
                    verified: true,
                })
            }
            AndroidOperation::RestartXiaoService => {
                let status = tokio::select! {
                    _ = cancellation.cancelled() => return Err(anyhow!("Android operation cancelled")),
                    result = Command::new(&self.setprop)
                        .args(["ctl.restart", self.service_name.as_str()])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::piped())
                        .kill_on_drop(true)
                        .output() => result?,
                };
                if !status.status.success() {
                    return Err(anyhow!(
                        "typed Xiao service restart failed: {}",
                        redact_text(&String::from_utf8_lossy(&status.stderr))
                    ));
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
                let state = self.status(&cancellation).await?;
                let verified = state == "running";
                Ok(AndroidOutcome {
                    operation,
                    succeeded: true,
                    evidence: format!("restart requested; init.svc.{}={state}", self.service_name),
                    verified,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeBroker {
        operations: Mutex<Vec<AndroidOperation>>,
    }

    #[async_trait]
    impl AndroidBroker for FakeBroker {
        async fn execute(
            &self,
            operation: AndroidOperation,
            _: CancellationToken,
        ) -> Result<AndroidOutcome> {
            self.operations.lock().unwrap().push(operation.clone());
            Ok(AndroidOutcome {
                operation,
                succeeded: true,
                evidence: "fake init state is running".into(),
                verified: true,
            })
        }
    }

    #[tokio::test]
    async fn broker_surface_is_typed_and_restart_is_approval_classed() {
        let broker = FakeBroker {
            operations: Mutex::new(Vec::new()),
        };
        let outcome = broker
            .execute(
                AndroidOperation::RestartXiaoService,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(outcome.verified);
        assert!(outcome.operation.requires_approval());
        assert_eq!(outcome.operation.capability(), "android.service.restart");
    }
}
