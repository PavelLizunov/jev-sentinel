use crate::core::models::{TargetStatus, TargetTelemetry};
use chrono::Utc;
use reqwest::Client;
use serde_json::json;
use std::time::{Duration, Instant};
use tracing::debug;

pub async fn collect_http(
    name: String,
    url: String,
    expected_status: u16,
    timeout_seconds: u64,
    headers: std::collections::HashMap<String, String>,
) -> TargetTelemetry {
    let t0 = Instant::now();
    let client = match Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return TargetTelemetry {
                target_name: name,
                target_type: "http_probe".to_string(),
                status: TargetStatus::Unreachable,
                latency_ms: 0.0,
                metrics: json!({ "error": e.to_string() }),
                error_message: Some(e.to_string()),
                timestamp: Utc::now(),
            }
        }
    };

    let mut req = client.get(&url);
    for (k, v) in headers {
        req = req.header(k, v);
    }

    match req.send().await {
        Ok(resp) => {
            let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let status_code = resp.status().as_u16();
            let is_match = status_code == expected_status;

            let status = if is_match {
                TargetStatus::Online
            } else if status_code >= 500 {
                TargetStatus::Degraded
            } else {
                TargetStatus::Degraded
            };

            debug!(
                "HTTP probe '{}' -> {} (expected {}), latency {:.1}ms",
                name, status_code, expected_status, latency_ms
            );

            TargetTelemetry {
                target_name: name,
                target_type: "http_probe".to_string(),
                status,
                latency_ms,
                metrics: json!({
                    "url": url,
                    "status_code": status_code,
                    "expected_status": expected_status,
                    "matched": is_match
                }),
                error_message: if is_match {
                    None
                } else {
                    Some(format!("Unexpected HTTP status {}", status_code))
                },
                timestamp: Utc::now(),
            }
        }
        Err(e) => {
            let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let status = if e.is_timeout() {
                TargetStatus::Timeout
            } else {
                TargetStatus::Unreachable
            };

            TargetTelemetry {
                target_name: name,
                target_type: "http_probe".to_string(),
                status,
                latency_ms,
                metrics: json!({
                    "url": url,
                    "error": e.to_string(),
                    "is_timeout": e.is_timeout()
                }),
                error_message: Some(e.to_string()),
                timestamp: Utc::now(),
            }
        }
    }
}
