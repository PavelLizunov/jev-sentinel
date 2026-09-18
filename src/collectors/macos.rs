use crate::core::models::{TargetStatus, TargetTelemetry};
use chrono::Utc;
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::debug;

pub async fn collect_macos(
    name: String,
    host: String,
    user: String,
    ssh_key: Option<String>,
    port: Option<u16>,
    timeout_seconds: u64,
    check_services: Vec<String>,
) -> TargetTelemetry {
    let t0 = Instant::now();

    let mut ssh_args = vec![
        "-o".to_string(), "BatchMode=yes".to_string(),
        "-o".to_string(), "StrictHostKeyChecking=no".to_string(),
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

    // Shell payload to execute on macOS
    let probe_cmd = "sysctl vm.swapusage; echo '===VM_STAT==='; vm_stat | head -8; echo '===PROCESSES==='; ps aux";
    ssh_args.push(probe_cmd.to_string());

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
                    target_type: "macos_ssh".to_string(),
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

            let parsed_metrics = parse_macos_probe(&stdout, &check_services);
            debug!("macOS probe '{}' succeeded in {:.1}ms: {:?}", name, latency_ms, parsed_metrics);

            let swap_used = parsed_metrics["swap_used_mb"].as_f64().unwrap_or(0.0);
            let status = if swap_used >= 6000.0 {
                TargetStatus::Degraded
            } else {
                TargetStatus::Online
            };

            TargetTelemetry {
                target_name: name,
                target_type: "macos_ssh".to_string(),
                status,
                latency_ms,
                metrics: parsed_metrics,
                error_message: None,
                timestamp: Utc::now(),
            }
        }
        Ok(Err(e)) => TargetTelemetry {
            target_name: name,
            target_type: "macos_ssh".to_string(),
            status: TargetStatus::Unreachable,
            latency_ms,
            metrics: json!({ "error": e.to_string() }),
            error_message: Some(e.to_string()),
            timestamp: Utc::now(),
        },
        Err(_) => TargetTelemetry {
            target_name: name,
            target_type: "macos_ssh".to_string(),
            status: TargetStatus::Timeout,
            latency_ms,
            metrics: json!({ "timeout": true }),
            error_message: Some(format!("SSH timed out after {}s", timeout_seconds)),
            timestamp: Utc::now(),
        },
    }
}

fn parse_macos_probe(raw: &str, check_services: &[String]) -> serde_json::Value {
    let mut swap_used_mb = 0.0;
    let mut swap_total_mb = 0.0;
    let mut swap_free_mb = 0.0;

    let mut pages_free: u64 = 0;
    let mut pages_active: u64 = 0;
    let mut pages_inactive: u64 = 0;
    let mut pages_wired: u64 = 0;

    for line in raw.lines() {
        let l = line.trim();
        if l.starts_with("vm.swapusage:") {
            // vm.swapusage: total = 2048.00M  used = 710.81M  free = 1337.19M
            for part in l.split_whitespace() {
                if let Some(rest) = part.strip_suffix('M') {
                    if let Ok(val) = rest.parse::<f64>() {
                        if swap_total_mb == 0.0 {
                            swap_total_mb = val;
                        } else if swap_used_mb == 0.0 {
                            swap_used_mb = val;
                        } else if swap_free_mb == 0.0 {
                            swap_free_mb = val;
                        }
                    }
                }
            }
        } else if l.starts_with("Pages free:") {
            pages_free = parse_stat_num(l);
        } else if l.starts_with("Pages active:") {
            pages_active = parse_stat_num(l);
        } else if l.starts_with("Pages inactive:") {
            pages_inactive = parse_stat_num(l);
        } else if l.starts_with("Pages wired down:") {
            pages_wired = parse_stat_num(l);
        }
    }

    // Convert page counts to MB (16KB per page on Apple Silicon)
    let page_size_mb = 16384.0 / (1024.0 * 1024.0);
    let free_ram_mb = (pages_free + pages_inactive) as f64 * page_size_mb;
    let wired_ram_mb = pages_wired as f64 * page_size_mb;
    let active_ram_mb = pages_active as f64 * page_size_mb;

    // Service presence checks
    let mut service_status = serde_json::Map::new();
    for svc in check_services {
        let is_running = raw.contains(svc.as_str());
        service_status.insert(svc.clone(), json!(is_running));
    }

    json!({
        "swap_total_mb": swap_total_mb,
        "swap_used_mb": swap_used_mb,
        "swap_free_mb": swap_free_mb,
        "free_ram_mb": (free_ram_mb * 10.0).round() / 10.0,
        "wired_ram_mb": (wired_ram_mb * 10.0).round() / 10.0,
        "active_ram_mb": (active_ram_mb * 10.0).round() / 10.0,
        "services": service_status
    })
}

fn parse_stat_num(line: &str) -> u64 {
    line.split(':')
        .nth(1)
        .unwrap_or_default()
        .trim()
        .trim_end_matches('.')
        .parse::<u64>()
        .unwrap_or(0)
}
