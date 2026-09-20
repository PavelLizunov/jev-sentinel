use crate::core::models::{TargetStatus, TargetTelemetry};
use chrono::Utc;
use reqwest::Client;
use serde_json::json;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tracing::debug;

pub const MAX_BODY_DRAIN_BYTES: usize = 65536; // 64 KiB
pub const BODY_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DrainResult {
    Complete(usize),
    BudgetReached(usize),
    Timeout,
    ReadError,
}

pub async fn drain_response_body_bounded(mut resp: reqwest::Response) -> DrainResult {
    let mut total_drained = 0usize;
    let deadline = tokio::time::sleep(BODY_DRAIN_TIMEOUT);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => {
                return DrainResult::Timeout;
            }
            chunk_res = resp.chunk() => {
                match chunk_res {
                    Ok(Some(chunk)) => {
                        total_drained = total_drained.saturating_add(chunk.len());
                        if total_drained >= MAX_BODY_DRAIN_BYTES {
                            return DrainResult::BudgetReached(total_drained);
                        }
                    }
                    Ok(None) => {
                        return DrainResult::Complete(total_drained);
                    }
                    Err(_) => {
                        return DrainResult::ReadError;
                    }
                }
            }
        }
    }
}

static HTTP_CLIENT_POOL: OnceLock<Client> = OnceLock::new();

fn get_shared_client() -> &'static Client {
    HTTP_CLIENT_POOL.get_or_init(|| {
        Client::builder()
            .pool_max_idle_per_host(8)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

pub async fn collect_http(
    name: String,
    url: String,
    expected_status: u16,
    timeout_seconds: u64,
    headers: std::collections::HashMap<String, String>,
) -> TargetTelemetry {
    let t0 = Instant::now();
    let client = get_shared_client();

    let mut req = client
        .get(&url)
        .timeout(Duration::from_secs(timeout_seconds));
    for (k, v) in headers {
        req = req.header(k, v);
    }

    match req.send().await {
        Ok(resp) => {
            let latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let status_code = resp.status().as_u16();
            let drain_result = drain_response_body_bounded(resp).await;
            let is_match = status_code == expected_status;

            let status = if is_match {
                TargetStatus::Online
            } else if status_code >= 500 {
                TargetStatus::Unreachable
            } else {
                TargetStatus::Degraded
            };

            debug!(
                "HTTP probe '{}' -> {} (expected {}), latency {:.1}ms, drain {:?}",
                name, status_code, expected_status, latency_ms, drain_result
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
                    "matched": is_match,
                    "drain": drain_result
                }),
                error_message: if is_match {
                    None
                } else {
                    Some(format!("Unexpected HTTP status {}", status_code))
                },
                timestamp: Utc::now(),
                observed_at: Some(Instant::now()),
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
                observed_at: Some(Instant::now()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_drain_small_response_complete() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let resp =
                b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nHello World!";
            socket.write_all(resp).await.unwrap();
        });

        let client = Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}"))
            .send()
            .await
            .unwrap();
        let drain = drain_response_body_bounded(resp).await;
        assert_eq!(drain, DrainResult::Complete(12));
    }

    #[tokio::test]
    async fn test_drain_budget_reached_stops_consuming() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let header =
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            socket.write_all(header).await.unwrap();
            // Chunk 1: 60 KiB
            let chunk_size = 60 * 1024;
            socket
                .write_all(format!("{:X}\r\n", chunk_size).as_bytes())
                .await
                .unwrap();
            socket.write_all(&vec![b'A'; chunk_size]).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
            // Chunk 2: 16 KiB (crosses 64 KiB threshold)
            let chunk_size_2 = 16 * 1024;
            socket
                .write_all(format!("{:X}\r\n", chunk_size_2).as_bytes())
                .await
                .unwrap();
            socket.write_all(&vec![b'B'; chunk_size_2]).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
            // Server keeps socket open; client must break immediately without hanging
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        let client = Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}"))
            .send()
            .await
            .unwrap();
        let drain = drain_response_body_bounded(resp).await;
        match drain {
            DrainResult::BudgetReached(bytes) => {
                assert!(bytes >= MAX_BODY_DRAIN_BYTES);
            }
            other => panic!("Expected BudgetReached, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_drain_slow_body_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let header =
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            socket.write_all(header).await.unwrap();
            // Send 1 byte and sleep longer than BODY_DRAIN_TIMEOUT (500ms)
            socket.write_all(b"1\r\nX\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_millis(700)).await;
        });

        let client = Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}"))
            .send()
            .await
            .unwrap();
        let drain = drain_response_body_bounded(resp).await;
        assert_eq!(drain, DrainResult::Timeout);
    }
}
