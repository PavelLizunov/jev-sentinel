use crate::core::models::{TargetStatus, TargetTelemetry};
use chrono::Utc;
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::debug;

pub async fn collect_nvidia(
    name: String,
    host: String,
    user: String,
    ssh_key: Option<String>,
    port: Option<u16>,
    timeout_seconds: u64,
) -> TargetTelemetry {
    let t0 = Instant::now();

    let mut ssh_args = vec![
        "-o".to_string(), "BatchMode=yes".to_string(),
        "-o".to_string(), "StrictHostKeyChecking=yes".to_string(),
        "-o".to_string(), format!("ConnectTimeout={}", timeout_seconds),
    ];

    if let Some(p) = port {
        ssh_args.push("-p".to_string());
        ssh_args.push(p.to_string());
    }

    if let Some(key) = &ssh_key {
        let expanded = crate::config::interpolate_env_vars(key);
        ssh_args.push("-i".to_string());
        ssh_args.push(expanded);
    }

    let target_dest = format!("{}@{}", user, host);
    ssh_args.push(target_dest);

    let query_cmd = "nvidia-smi --query-gpu=name,temperature.gpu,memory.used,memory.total,utilization.gpu --format=csv,noheader,nounits";
    ssh_args.push(query_cmd.to_string());

    let mut cmd = Command::new("ssh");
    cmd.args(&ssh_args);

    let res = timeout(Duration::from_secs(timeout_seconds + 3), cmd.output()).await;
    let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;

    match res {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !output.status.success() {
                return TargetTelemetry {
                    target_name: name,
                    target_type: "nvidia_ssh".to_string(),
                    status: TargetStatus::Unreachable,
                    latency_ms,
                    metrics: json!({
                        "error": stderr.trim(),
                        "exit_code": output.status.code()
                    }),
                    error_message: Some(stderr.trim().to_string()),
                    timestamp: Utc::now(),
                };
            }

            let parsed_metrics = parse_nvidia_csv(&stdout);
            debug!("Nvidia GPU probe '{}' succeeded in {:.1}ms: {:?}", name, latency_ms, parsed_metrics);

            let temp_c = parsed_metrics["temperature_c"].as_f64().unwrap_or(0.0);
            let mem_used = parsed_metrics["memory_used_mb"].as_f64().unwrap_or(0.0);
            let mem_total = parsed_metrics["memory_total_mb"].as_f64().unwrap_or(1.0);
            let mem_ratio = mem_used / mem_total;

            let status = if temp_c >= 85.0 || mem_ratio >= 0.98 {
                TargetStatus::Degraded
            } else {
                TargetStatus::Online
            };

            TargetTelemetry {
                target_name: name,
                target_type: "nvidia_ssh".to_string(),
                status,
                latency_ms,
                metrics: parsed_metrics,
                error_message: None,
                timestamp: Utc::now(),
            }
        }
        Ok(Err(e)) => TargetTelemetry {
            target_name: name,
            target_type: "nvidia_ssh".to_string(),
            status: TargetStatus::Unreachable,
            latency_ms,
            metrics: json!({ "error": e.to_string() }),
            error_message: Some(e.to_string()),
            timestamp: Utc::now(),
        },
        Err(_) => TargetTelemetry {
            target_name: name,
            target_type: "nvidia_ssh".to_string(),
            status: TargetStatus::Timeout,
            latency_ms,
            metrics: json!({ "timeout": true }),
            error_message: Some(format!("SSH timed out after {}s", timeout_seconds)),
            timestamp: Utc::now(),
        },
    }
}

fn parse_nvidia_csv(raw: &str) -> serde_json::Value {
    // Expected output: "NVIDIA GeForce RTX 5060 Ti, 29, 14040, 16384, 0"
    if let Some(line) = raw.lines().find(|l| !l.trim().is_empty()) {
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() >= 5 {
            let name = parts[0];
            let temp: f64 = parts[1].parse().unwrap_or(0.0);
            let mem_used: f64 = parts[2].parse().unwrap_or(0.0);
            let mem_total: f64 = parts[3].parse().unwrap_or(0.0);
            let util: f64 = parts[4].parse().unwrap_or(0.0);
            let mem_free = if mem_total >= mem_used {
                mem_total - mem_used
            } else {
                0.0
            };

            return json!({
                "gpu_name": name,
                "temperature_c": temp,
                "memory_used_mb": mem_used,
                "memory_total_mb": mem_total,
                "memory_free_mb": mem_free,
                "utilization_gpu_pct": util
            });
        }
    }

    json!({ "raw_output": raw.trim() })
}
