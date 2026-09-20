pub mod exec;
pub mod http;
pub mod macos;
pub mod nvidia;
pub mod runner;
pub mod tcp;

use crate::config::TargetConfig;
use crate::core::models::{InfrastructureSnapshot, TargetStatus, TargetTelemetry};
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::info;

pub async fn collect_target(target: &TargetConfig) -> TargetTelemetry {
    match target {
        TargetConfig::HttpProbe {
            name,
            url,
            expected_status,
            timeout_seconds,
            headers,
        } => {
            http::collect_http(
                name.clone(),
                url.clone(),
                *expected_status,
                *timeout_seconds,
                headers.clone(),
            )
            .await
        }
        TargetConfig::TcpPing {
            name,
            host,
            port,
            timeout_seconds,
        } => tcp::collect_tcp(name.clone(), host.clone(), *port, *timeout_seconds).await,
        TargetConfig::ExecProbe {
            name,
            command,
            timeout_seconds,
        } => exec::collect_exec(name.clone(), command.clone(), *timeout_seconds).await,
        TargetConfig::MacosSsh {
            name,
            host,
            user,
            ssh_key,
            port,
            timeout_seconds,
            check_services,
        } => {
            macos::collect_macos(
                name.clone(),
                host.clone(),
                user.clone(),
                ssh_key.clone(),
                *port,
                *timeout_seconds,
                check_services.clone(),
            )
            .await
        }
        TargetConfig::NvidiaSsh {
            name,
            host,
            user,
            ssh_key,
            port,
            timeout_seconds,
        } => {
            nvidia::collect_nvidia(
                name.clone(),
                host.clone(),
                user.clone(),
                ssh_key.clone(),
                *port,
                *timeout_seconds,
            )
            .await
        }
    }
}

pub async fn collect_all(targets: &[TargetConfig], concurrency: usize) -> InfrastructureSnapshot {
    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut tasks = Vec::with_capacity(targets.len());

    for target in targets {
        let sem = semaphore.clone();
        let target_clone = target.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            collect_target(&target_clone).await
        }));
    }

    let mut telemetries = Vec::with_capacity(tasks.len());
    for (i, task) in tasks.into_iter().enumerate() {
        match task.await {
            Ok(telemetry) => telemetries.push(telemetry),
            Err(join_err) => {
                let target = &targets[i];
                telemetries.push(TargetTelemetry {
                    target_name: target.name().to_string(),
                    target_type: target.type_name().to_string(),
                    status: TargetStatus::Unknown,
                    latency_ms: 0.0,
                    metrics: serde_json::json!({ "internal_error": join_err.to_string() }),
                    error_message: Some(format!("Collector task join error: {}", join_err)),
                    timestamp: Utc::now(),
                    observed_at: Some(std::time::Instant::now()),
                });
            }
        }
    }

    info!(
        "Collected telemetry from {}/{} targets",
        telemetries.len(),
        targets.len()
    );

    InfrastructureSnapshot {
        timestamp: Utc::now(),
        targets: telemetries,
    }
}
