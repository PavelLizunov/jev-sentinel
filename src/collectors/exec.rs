use crate::core::models::{TargetStatus, TargetTelemetry};
use chrono::Utc;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::debug;

pub async fn collect_exec(
    name: String,
    command: Vec<String>,
    timeout_seconds: u64,
) -> TargetTelemetry {
    let t0 = Instant::now();
    if command.is_empty() {
        return TargetTelemetry {
            target_name: name,
            target_type: "exec_probe".to_string(),
            status: TargetStatus::Degraded,
            latency_ms: 0.0,
            metrics: json!({ "error": "Empty command vector" }),
            error_message: Some("Empty command vector".to_string()),
            timestamp: Utc::now(),
        };
    }

    let program = &command[0];
    let args = &command[1..];

    let mut cmd = Command::new(program);
    cmd.args(args);

    let res = timeout(Duration::from_secs(timeout_seconds), cmd.output()).await;
    let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;

    match res {
        Ok(Ok(output)) => {
            let stdout_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr_str = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let exit_code = output.status.code().unwrap_or(-1);

            let status = if output.status.success() {
                TargetStatus::Online
            } else {
                TargetStatus::Degraded
            };

            let parsed_json: Option<Value> = serde_json::from_str(&stdout_str).ok();
            let metrics = match parsed_json {
                Some(val) => val,
                None => json!({
                    "raw_output": stdout_str,
                    "stderr": stderr_str,
                    "exit_code": exit_code
                }),
            };

            debug!(
                "Exec probe '{}' finished with code {} in {:.1}ms",
                name, exit_code, latency_ms
            );

            TargetTelemetry {
                target_name: name,
                target_type: "exec_probe".to_string(),
                status,
                latency_ms,
                metrics,
                error_message: if output.status.success() {
                    None
                } else {
                    Some(format!("Command exited with status code {}", exit_code))
                },
                timestamp: Utc::now(),
            }
        }
        Ok(Err(e)) => {
            debug!("Exec probe '{}' failed to start: {}", name, e);
            TargetTelemetry {
                target_name: name,
                target_type: "exec_probe".to_string(),
                status: TargetStatus::Unreachable,
                latency_ms,
                metrics: json!({ "error": e.to_string() }),
                error_message: Some(e.to_string()),
                timestamp: Utc::now(),
            }
        }
        Err(_) => {
            debug!("Exec probe '{}' timed out after {}s", name, timeout_seconds);
            TargetTelemetry {
                target_name: name,
                target_type: "exec_probe".to_string(),
                status: TargetStatus::Timeout,
                latency_ms,
                metrics: json!({ "timeout": true }),
                error_message: Some(format!("Command timed out after {}s", timeout_seconds)),
                timestamp: Utc::now(),
            }
        }
    }
}
