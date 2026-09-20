use crate::collectors::runner::{
    run_bounded_process, DEFAULT_MAX_STDERR_BYTES, DEFAULT_MAX_STDOUT_BYTES,
};
use crate::core::models::{TargetStatus, TargetTelemetry};
use chrono::Utc;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
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
            status: TargetStatus::Unknown,
            latency_ms: 0.0,
            metrics: json!({ "error": "Empty command vector" }),
            error_message: Some("Empty command vector".to_string()),
            timestamp: Utc::now(),
            observed_at: Some(Instant::now()),
        };
    }

    let program = &command[0];
    let args = &command[1..];

    let res = run_bounded_process(
        program,
        args,
        Duration::from_secs(timeout_seconds),
        DEFAULT_MAX_STDOUT_BYTES,
        DEFAULT_MAX_STDERR_BYTES,
    )
    .await;
    let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;

    match res {
        Ok(proc_out) => {
            if proc_out.timed_out {
                debug!("Exec probe '{}' timed out after {}s", name, timeout_seconds);
                return TargetTelemetry {
                    target_name: name,
                    target_type: "exec_probe".to_string(),
                    status: TargetStatus::Timeout,
                    latency_ms,
                    metrics: json!({ "timeout": true }),
                    error_message: Some(format!("Command timed out after {}s", timeout_seconds)),
                    timestamp: Utc::now(),
                    observed_at: Some(Instant::now()),
                };
            }

            let parsed_json: Option<Value> =
                serde_json::from_str(&proc_out.stdout)
                    .ok()
                    .filter(|value: &Value| {
                        (value.is_object() || value.is_array()) && !proc_out.truncated
                    });
            let parse_error = parsed_json.is_none();
            let status = if parse_error {
                TargetStatus::Unknown
            } else if proc_out.success {
                TargetStatus::Online
            } else {
                TargetStatus::Degraded
            };
            let metrics = match parsed_json {
                Some(val) => val,
                None => json!({
                    "raw_output": proc_out.stdout,
                    "stderr": proc_out.stderr,
                    "exit_code": proc_out.exit_code,
                    "truncated": proc_out.truncated,
                    "parse_error": true
                }),
            };

            debug!(
                "Exec probe '{}' finished with code {} in {:.1}ms",
                name, proc_out.exit_code, latency_ms
            );

            TargetTelemetry {
                target_name: name,
                target_type: "exec_probe".to_string(),
                status,
                latency_ms,
                metrics,
                error_message: if parse_error {
                    Some("Probe did not return a complete JSON object or array".into())
                } else if proc_out.success {
                    None
                } else {
                    Some(format!(
                        "Command exited with status code {}",
                        proc_out.exit_code
                    ))
                },
                timestamp: Utc::now(),
                observed_at: Some(Instant::now()),
            }
        }
        Err(e) => {
            debug!("Exec probe '{}' failed to start: {}", name, e);
            TargetTelemetry {
                target_name: name,
                target_type: "exec_probe".to_string(),
                status: TargetStatus::Unreachable,
                latency_ms,
                metrics: json!({ "error": e.to_string() }),
                error_message: Some(e.to_string()),
                timestamp: Utc::now(),
                observed_at: Some(Instant::now()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn malformed_or_missing_exec_metrics_are_not_healthy() {
        for output in ["not-json", "null", "42"] {
            let result = collect_exec(
                "fixture".into(),
                vec!["printf".into(), "%s".into(), output.into()],
                2,
            )
            .await;
            assert_eq!(result.status, TargetStatus::Unknown);
            assert_eq!(result.metrics["parse_error"], true);
        }
        assert_eq!(
            collect_exec("fixture".into(), vec![], 2).await.status,
            TargetStatus::Unknown
        );
        let valid = collect_exec(
            "fixture".into(),
            vec!["printf".into(), "%s".into(), "{\"cpu_pct\":0}".into()],
            2,
        )
        .await;
        assert_eq!(valid.status, TargetStatus::Online);
        assert_eq!(valid.metrics["cpu_pct"], 0);
    }
}
