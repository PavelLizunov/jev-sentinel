use crate::collectors::runner::{
    run_bounded_process, DEFAULT_MAX_STDERR_BYTES, DEFAULT_MAX_STDOUT_BYTES,
};
use crate::core::models::{TargetStatus, TargetTelemetry};
use chrono::Utc;
use serde_json::json;
use std::time::{Duration, Instant};
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

    // Shell payload to execute on macOS
    let probe_cmd =
        "sysctl hw.memsize vm.swapusage; echo '===VM_STAT==='; vm_stat; echo '===PROCESSES==='; ps aux";
    ssh_args.push(probe_cmd.to_string());

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
                    target_type: "macos_ssh".to_string(),
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
                    target_type: "macos_ssh".to_string(),
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

            let parsed_metrics = parse_macos_probe(&proc_out.stdout, &check_services);
            debug!(
                "macOS probe '{}' succeeded in {:.1}ms: {:?}",
                name, latency_ms, parsed_metrics
            );

            // This is an available-memory estimate, not a pressure forecast. Allocated
            // swap/free swap pool says nothing about remaining filesystem capacity.
            let status = match parsed_metrics["free_ram_mb"].as_f64() {
                Some(free) if free < 800.0 => TargetStatus::Degraded,
                Some(_) => TargetStatus::Online,
                None => TargetStatus::Unknown,
            };

            TargetTelemetry {
                target_name: name,
                target_type: "macos_ssh".to_string(),
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
            target_type: "macos_ssh".to_string(),
            status: TargetStatus::Unreachable,
            latency_ms,
            metrics: json!({ "error": e.to_string() }),
            error_message: Some(e.to_string()),
            timestamp: Utc::now(),
            observed_at: Some(Instant::now()),
        },
    }
}

pub fn parse_macos_probe(raw: &str, check_services: &[String]) -> serde_json::Value {
    let mut swap_used_mb = None;
    let mut swap_total_mb = None;
    let mut swap_free_mb = None;
    let mut physical_memory_mb = None;
    let mut pages_free = None;
    let mut pages_active = None;
    let mut pages_inactive = None;
    let mut pages_wired = None;
    let mut page_size_bytes = None;

    for line in raw.lines() {
        let l = line.trim();
        if let Some(value) = l.strip_prefix("hw.memsize:") {
            physical_memory_mb = value
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .map(|n| n as f64 / 1048576.0);
        } else if l.starts_with("vm.swapusage:") {
            // vm.swapusage: total = 2048.00M  used = 710.81M  free = 1337.19M
            let parts: Vec<&str> = l.split_whitespace().collect();
            for i in 0..parts.len() {
                if parts[i] == "total" && i + 2 < parts.len() && parts[i + 1] == "=" {
                    if let Some(rest) = parts[i + 2].strip_suffix('M') {
                        if let Ok(v) = rest.parse::<f64>() {
                            swap_total_mb = Some(v);
                        }
                    }
                } else if parts[i] == "used" && i + 2 < parts.len() && parts[i + 1] == "=" {
                    if let Some(rest) = parts[i + 2].strip_suffix('M') {
                        if let Ok(v) = rest.parse::<f64>() {
                            swap_used_mb = Some(v);
                        }
                    }
                } else if parts[i] == "free" && i + 2 < parts.len() && parts[i + 1] == "=" {
                    if let Some(rest) = parts[i + 2].strip_suffix('M') {
                        if let Ok(v) = rest.parse::<f64>() {
                            swap_free_mb = Some(v);
                        }
                    }
                }
            }
        } else if l.contains("page size of") {
            if let Some(start) = l.find("page size of ") {
                let rest = &l[start + "page size of ".len()..];
                if let Some(num_str) = rest.split_whitespace().next() {
                    if let Ok(num) = num_str.parse::<f64>() {
                        page_size_bytes = Some(num);
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

    // Convert page counts to MB
    let page_size_mb = page_size_bytes
        .filter(|n: &f64| n.is_finite() && *n > 0.0)
        .map(|n| n / 1048576.0);
    let to_mib = |pages: Option<u64>| {
        pages
            .zip(page_size_mb)
            .map(|(p, size)| (p as f64 * size * 10.0).round() / 10.0)
    };
    let free_ram_mb = to_mib(
        pages_free
            .zip(pages_inactive)
            .and_then(|(a, b)| a.checked_add(b)),
    );
    let wired_ram_mb = to_mib(pages_wired);
    let active_ram_mb = to_mib(pages_active);
    let valid_swap = |v: Option<f64>| v.filter(|n| n.is_finite() && *n >= 0.0);

    // Service presence checks
    let mut service_status = serde_json::Map::new();
    for svc in check_services {
        let is_running = raw.contains(svc.as_str());
        service_status.insert(svc.clone(), json!(is_running));
    }

    json!({
        "physical_memory_mb": physical_memory_mb,
        "swap_total_mb": valid_swap(swap_total_mb),
        "swap_used_mb": valid_swap(swap_used_mb),
        "swap_free_mb": valid_swap(swap_free_mb),
        "free_ram_mb": free_ram_mb,
        "wired_ram_mb": wired_ram_mb,
        "active_ram_mb": active_ram_mb,
        "parse_error": free_ram_mb.is_none(),
        "services": service_status
    })
}

fn parse_stat_num(line: &str) -> Option<u64> {
    line.split(':')
        .nth(1)
        .unwrap_or_default()
        .trim()
        .trim_end_matches('.')
        .parse::<u64>()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_metrics_stay_null_and_real_capacity_is_used() {
        let missing = parse_macos_probe("permission denied", &[]);
        assert!(missing["free_ram_mb"].is_null());
        assert!(missing["swap_used_mb"].is_null());
        assert_eq!(missing["parse_error"], true);
        let actual = parse_macos_probe("hw.memsize: 34359738368\n", &[]);
        assert_eq!(actual["physical_memory_mb"], 32768.0);
        let no_page_size = parse_macos_probe("Pages free: 1.\nPages inactive: 2.\n", &[]);
        assert!(no_page_size["free_ram_mb"].is_null());
    }

    #[test]
    fn test_parse_macos_probe_zero_swap_preservation() {
        let sample = "\
vm.swapusage: total = 3072.00M  used = 0.00M  free = 3072.00M  (encrypted)
===VM_STAT===
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               125000.
Pages active:                             250000.
Pages inactive:                           100000.
Pages wired down:                         150000.
===PROCESSES===
slovn 100 0.0 osaurus
";
        let metrics = parse_macos_probe(sample, &["osaurus".to_string(), "gemma".to_string()]);
        assert_eq!(metrics["swap_total_mb"], 3072.0);
        assert_eq!(metrics["swap_used_mb"], 0.0);
        assert_eq!(metrics["swap_free_mb"], 3072.0);
        assert_eq!(metrics["services"]["osaurus"], true);
        assert_eq!(metrics["services"]["gemma"], false);
    }

    #[test]
    fn test_parse_macos_probe_intel_page_size() {
        let sample = "\
vm.swapusage: total = 1024.00M  used = 512.00M  free = 512.00M
===VM_STAT===
Mach Virtual Memory Statistics: (page size of 4096 bytes)
Pages free:                               100000.
Pages active:                             100000.
Pages inactive:                           100000.
Pages wired down:                         100000.
===PROCESSES===
";
        let metrics = parse_macos_probe(sample, &[]);
        assert_eq!(metrics["swap_used_mb"], 512.0);
        // (100000 + 100000) * 4096 / (1024*1024) = 781.25 MB ~ 781.3 MB
        assert_eq!(metrics["free_ram_mb"], 781.3);
    }
}
