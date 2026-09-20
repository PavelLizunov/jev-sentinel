use crate::collectors::runner::{
    run_bounded_process, DEFAULT_MAX_STDERR_BYTES, DEFAULT_MAX_STDOUT_BYTES,
};
use crate::core::models::{TargetStatus, TargetTelemetry};
use chrono::Utc;
use serde_json::json;
use std::time::{Duration, Instant};
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
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=yes".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={}", timeout_seconds),
    ];

    if let Some(p) = port {
        ssh_args.push("-p".to_string());
        ssh_args.push(p.to_string());
    }

    if let Some(key) = &ssh_key {
        let expanded = crate::config::interpolate_env_vars(key);
        let normalized = if let Some(stripped) = expanded.strip_prefix("~/") {
            if let Ok(home) = std::env::var("HOME") {
                format!("{}/{}", home, stripped)
            } else {
                expanded
            }
        } else {
            expanded
        };
        ssh_args.push("-i".to_string());
        ssh_args.push(normalized);
    }

    let target_dest = format!("{}@{}", user, host);
    ssh_args.push("--".to_string());
    ssh_args.push(target_dest);

    let query_cmd = "nvidia-smi --query-gpu=name,temperature.gpu,memory.used,memory.total,utilization.gpu --format=csv,noheader,nounits";
    ssh_args.push(query_cmd.to_string());

    let res = run_bounded_process(
        "ssh",
        &ssh_args,
        Duration::from_secs(timeout_seconds + 3),
        DEFAULT_MAX_STDOUT_BYTES,
        DEFAULT_MAX_STDERR_BYTES,
    )
    .await;
    let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;

    match res {
        Ok(proc_out) => {
            if proc_out.timed_out {
                return TargetTelemetry {
                    target_name: name,
                    target_type: "nvidia_ssh".to_string(),
                    status: TargetStatus::Timeout,
                    latency_ms,
                    metrics: json!({ "timeout": true }),
                    error_message: Some(format!("SSH timed out after {}s", timeout_seconds)),
                    timestamp: Utc::now(),
                    observed_at: Some(Instant::now()),
                };
            }

            if !proc_out.success {
                return TargetTelemetry {
                    target_name: name,
                    target_type: "nvidia_ssh".to_string(),
                    status: TargetStatus::Unreachable,
                    latency_ms,
                    metrics: json!({
                        "error": proc_out.stderr.trim(),
                        "exit_code": proc_out.exit_code
                    }),
                    error_message: Some(proc_out.stderr.trim().to_string()),
                    timestamp: Utc::now(),
                    observed_at: Some(Instant::now()),
                };
            }

            let parsed_metrics = parse_nvidia_csv(&proc_out.stdout);
            debug!(
                "Nvidia GPU probe '{}' succeeded in {:.1}ms: {:?}",
                name, latency_ms, parsed_metrics
            );

            if parsed_metrics.get("error").is_some() {
                return TargetTelemetry {
                    target_name: name,
                    target_type: "nvidia_ssh".to_string(),
                    status: TargetStatus::Unknown,
                    latency_ms,
                    metrics: parsed_metrics,
                    error_message: Some("Failed to parse nvidia-smi output".to_string()),
                    timestamp: Utc::now(),
                    observed_at: Some(Instant::now()),
                };
            }

            let temp_c = parsed_metrics["temperature_c"].as_f64().unwrap_or(0.0);
            let mem_used = parsed_metrics["memory_used_mb"].as_f64().unwrap_or(0.0);
            let mem_total = parsed_metrics["memory_total_mb"].as_f64().unwrap_or(1.0);
            let mem_ratio = if mem_total > 0.0 {
                mem_used / mem_total
            } else {
                0.0
            };

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
                observed_at: Some(Instant::now()),
            }
        }
        Err(e) => TargetTelemetry {
            target_name: name,
            target_type: "nvidia_ssh".to_string(),
            status: TargetStatus::Unreachable,
            latency_ms,
            metrics: json!({ "error": e.to_string() }),
            error_message: Some(e.to_string()),
            timestamp: Utc::now(),
            observed_at: Some(Instant::now()),
        },
    }
}

pub fn parse_nvidia_csv(raw: &str) -> serde_json::Value {
    let mut gpus = Vec::new();
    let mut max_temp: f64 = 0.0;
    let mut total_mem_used: f64 = 0.0;
    let mut total_mem_total: f64 = 0.0;
    let mut max_util: f64 = 0.0;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split(',').map(|s| s.trim()).collect();
        if parts.len() < 5 {
            return json!({"error":"Incomplete GPU row", "parse_error":true});
        }
        if parts.len() >= 5 {
            let name = parts[0];
            let values: Option<Vec<f64>> = parts[1..5]
                .iter()
                .map(|s| s.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0))
                .collect();
            let Some(values) = values else {
                return json!({"error":"Invalid GPU metric", "parse_error":true});
            };
            let (temp, mem_used, mem_total, util) = (values[0], values[1], values[2], values[3]);
            if mem_total <= 0.0 || mem_used > mem_total || util > 100.0 {
                return json!({"error":"Out-of-range GPU metric", "parse_error":true});
            }
            let mem_free = if mem_total >= mem_used {
                mem_total - mem_used
            } else {
                0.0
            };

            if temp > max_temp {
                max_temp = temp;
            }
            if util > max_util {
                max_util = util;
            }
            total_mem_used += mem_used;
            total_mem_total += mem_total;

            gpus.push(json!({
                "gpu_name": name,
                "temperature_c": temp,
                "memory_used_mb": mem_used,
                "memory_total_mb": mem_total,
                "memory_free_mb": mem_free,
                "utilization_pct": util,
            }));
        }
    }

    if gpus.is_empty() {
        return json!({ "error": "No GPU rows parsed from nvidia-smi", "raw": raw });
    }

    let first = &gpus[0];
    json!({
        "gpu_name": first["gpu_name"],
        "temperature_c": max_temp,
        "memory_used_mb": total_mem_used,
        "memory_total_mb": total_mem_total,
        "memory_free_mb": (total_mem_total - total_mem_used).max(0.0),
        "utilization_pct": max_util,
        "gpu_count": gpus.len(),
        "gpus": gpus
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nvidia_csv_multi_gpu() {
        let sample = "\
NVIDIA GeForce RTX 5060 Ti, 45, 14000, 16384, 95
NVIDIA GeForce RTX 4090, 62, 20000, 24576, 80
";
        let metrics = parse_nvidia_csv(sample);
        assert_eq!(metrics["gpu_count"], 2);
        assert_eq!(metrics["temperature_c"], 62.0); // max temp
        assert_eq!(metrics["memory_used_mb"], 34000.0); // sum
        assert_eq!(metrics["memory_total_mb"], 40960.0); // sum
        assert_eq!(metrics["utilization_pct"], 95.0); // max util
    }

    #[test]
    fn unavailable_gpu_metrics_are_not_zeroes() {
        for raw in [
            "GPU, N/A, 100, 1000, 10",
            "GPU, 40, 100, 1000, NaN",
            "GPU, 40, 1100, 1000, 10",
            "GPU, 40, 100, 1000, 10\ntruncated second GPU",
        ] {
            let metrics = parse_nvidia_csv(raw);
            assert_eq!(metrics["parse_error"], true);
            assert!(metrics.get("memory_used_mb").is_none());
        }
    }

    #[test]
    fn test_parse_nvidia_csv_empty() {
        let metrics = parse_nvidia_csv("");
        assert!(metrics.get("error").is_some());
    }
}
