use crate::core::models::{TargetStatus, TargetTelemetry};
use chrono::Utc;
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::debug;

pub async fn collect_tcp(
    name: String,
    host: String,
    port: u16,
    timeout_seconds: u64,
) -> TargetTelemetry {
    let t0 = Instant::now();
    let addr = format!("{}:{}", host, port);

    let res = timeout(
        Duration::from_secs(timeout_seconds),
        TcpStream::connect((host.as_str(), port)),
    )
    .await;

    let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;

    match res {
        Ok(Ok(_stream)) => {
            debug!(
                "TCP ping '{}' ({}) succeeded in {:.1}ms",
                name, addr, latency_ms
            );
            TargetTelemetry {
                target_name: name,
                target_type: "tcp_ping".to_string(),
                status: TargetStatus::Online,
                latency_ms,
                metrics: json!({
                    "host": host,
                    "port": port,
                    "connected": true
                }),
                error_message: None,
                timestamp: Utc::now(),
                observed_at: Some(Instant::now()),
            }
        }
        Ok(Err(e)) => {
            debug!("TCP ping '{}' ({}) failed: {}", name, addr, e);
            TargetTelemetry {
                target_name: name,
                target_type: "tcp_ping".to_string(),
                status: TargetStatus::Unreachable,
                latency_ms,
                metrics: json!({
                    "host": host,
                    "port": port,
                    "error": e.to_string()
                }),
                error_message: Some(e.to_string()),
                timestamp: Utc::now(),
                observed_at: Some(Instant::now()),
            }
        }
        Err(_) => {
            debug!(
                "TCP ping '{}' ({}) timed out after {}s",
                name, addr, timeout_seconds
            );
            TargetTelemetry {
                target_name: name,
                target_type: "tcp_ping".to_string(),
                status: TargetStatus::Timeout,
                latency_ms,
                metrics: json!({
                    "host": host,
                    "port": port,
                    "timeout": true
                }),
                error_message: Some(format!("Connection timed out after {}s", timeout_seconds)),
                timestamp: Utc::now(),
                observed_at: Some(Instant::now()),
            }
        }
    }
}
