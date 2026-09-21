//! Additional tests for the supplied f562c25 archive.
//! Prepared from source inspection; NOT compiled or run by this reviewer.
//! All HTTP requests use a loopback mock with dummy credentials.
//! The three regression_* tests are expected to FAIL on the reviewed source.
//! The two control_* tests are expected to PASS. These are predictions, not results.
use jev_sentinel::actions::notifier::{
    AlertEvent, AlertReason, AlertSeverity, AlertSource, DeduplicationKey, DeliveryState,
    GenerationRecord, IncidentId, IncidentState, Notifier,
};
use jev_sentinel::config::TelegramAlertSettings;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone)]
struct Captured {
    body: Value,
    at: Instant,
    response_status: u16,
}

enum Reply {
    Json(u16, Value),
    Close,
}

type Handler = Box<dyn Fn(&Value, usize) -> Reply + Send + Sync>;

struct Mock {
    base: String,
    seen: Arc<Mutex<Vec<Captured>>>,
    handler: Arc<Mutex<Handler>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Mock {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Mutex<Handler>> = Arc::new(Mutex::new(Box::new(|_, _| {
            Reply::Json(200, json!({"ok":true}))
        })));
        let saved = Arc::clone(&seen);
        let response = Arc::clone(&handler);
        let task = tokio::spawn(async move {
            let mut count = 0;
            while let Ok((mut socket, _)) = listener.accept().await {
                let read = async {
                    let mut bytes = Vec::new();
                    let mut chunk = [0_u8; 4096];
                    let (end, length) = loop {
                        let n = socket.read(&mut chunk).await.ok()?;
                        if n == 0 {
                            return None;
                        }
                        bytes.extend_from_slice(&chunk[..n]);
                        if bytes.len() > 128 * 1024 {
                            return None;
                        }
                        if let Some(p) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = std::str::from_utf8(&bytes[..p]).ok()?;
                            let len = headers
                                .lines()
                                .filter_map(|l| l.split_once(':'))
                                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))?
                                .1
                                .trim()
                                .parse::<usize>()
                                .ok()?;
                            if len > 128 * 1024 {
                                return None;
                            }
                            break (p + 4, len);
                        }
                    };
                    while bytes.len() < end + length {
                        let n = socket.read(&mut chunk).await.ok()?;
                        if n == 0 {
                            return None;
                        }
                        bytes.extend_from_slice(&chunk[..n]);
                    }
                    serde_json::from_slice::<Value>(&bytes[end..end + length]).ok()
                };
                let body = match tokio::time::timeout(Duration::from_secs(5), read).await {
                    Ok(Some(body)) => body,
                    _ => continue,
                };
                count += 1;
                let reply = (response.lock().unwrap())(&body, count);
                let status = match &reply {
                    Reply::Json(s, _) => *s,
                    Reply::Close => 0,
                };
                saved.lock().unwrap().push(Captured {
                    body,
                    at: Instant::now(),
                    response_status: status,
                });
                if let Reply::Json(status, payload) = reply {
                    // Always derive Content-Length from the actual serialized byte string.
                    let body = serde_json::to_string(&payload).unwrap();
                    let reason = match status {
                        200 => "OK",
                        400 => "Bad Request",
                        429 => "Too Many Requests",
                        _ => "Error",
                    };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len(),
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                }
            }
        });
        Self {
            base,
            seen,
            handler,
            task,
        }
    }

    fn notifier(&self) -> Arc<Notifier> {
        Arc::new(Notifier::new_with_api_base(
            Some(TelegramAlertSettings {
                bot_token: "review-dummy-token".into(),
                chat_id: "0".into(),
                min_severity: "info".into(),
                proxy: None,
            }),
            &self.base,
        ))
    }

    fn set(&self, f: impl Fn(&Value, usize) -> Reply + Send + Sync + 'static) {
        *self.handler.lock().unwrap() = Box::new(f);
    }

    fn captured(&self) -> Vec<Captured> {
        self.seen.lock().unwrap().clone()
    }
}

fn event(n: &Notifier, message: &str) -> AlertEvent {
    n.build_alert_event(
        AlertSource::LocalRule,
        Some("node-a".into()),
        AlertSeverity::Warning,
        AlertReason::ServiceCheckFailed,
        "Review",
        message,
    )
}

#[tokio::test]
async fn regression_long_single_line_is_not_silently_truncated_in_fallback() {
    let mock = Mock::start().await;
    mock.set(|body, _| {
        let text = body["text"].as_str().unwrap();
        if text.chars().count() > 4096 {
            Reply::Json(400, json!({"ok":false,"description":"message is too long"}))
        } else {
            Reply::Json(200, json!({"ok":true}))
        }
    });
    let n = mock.notifier();
    let e = event(&n, &"X".repeat(5000));
    n.dispatch_alert_event(&e)
        .await
        .expect("successful full delivery");
    let delivered_x: usize = mock
        .captured()
        .iter()
        .filter(|r| r.response_status == 200)
        .map(|r| {
            r.body["text"]
                .as_str()
                .unwrap()
                .chars()
                .filter(|c| *c == 'X')
                .count()
        })
        .sum();
    assert_eq!(
        delivered_x, 5000,
        "success must not silently discard the original message tail"
    );
}

#[tokio::test]
async fn regression_historical_partial_plan_survives_release_after_cancellation() {
    let mock = Mock::start().await;
    let n = mock.notifier();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let sender = Arc::clone(&n);
    mock.set(move |body, _| {
        if body["text"].as_str().unwrap().contains("BBBB") {
            // Recovery occurs while the old delivery is explicitly registered in-flight.
            sender.recover_target("node-a");
            stop_tx.send(true).unwrap();
            Reply::Close
        } else {
            Reply::Json(200, json!({"ok":true}))
        }
    });
    let text = format!("{}\n{}", "A".repeat(3500), "B".repeat(3500));
    let e = event(&n, &text);
    assert!(n.enqueue_alert(e));
    let old = n.pop_next_pending().unwrap();
    let formatted = n.format_alert_event(&old);
    assert!(n
        .send_alert_message(&old, &formatted, Some(&stop_rx))
        .await
        .is_err());
    // This is exactly the cleanup used by run_worker on send_alert_message failure.
    n.release_in_flight(&old);
    mock.set(|_, _| Reply::Json(200, json!({"ok":true})));
    n.send_alert_message(&old, &formatted, None)
        .await
        .expect("resume historical plan");
    let count_a = mock
        .captured()
        .iter()
        .filter(|r| r.body["text"].as_str().unwrap().contains("AAAA"))
        .count();
    assert_eq!(
        count_a, 1,
        "release/prune must not forget an acknowledged historical part"
    );
}

#[tokio::test]
async fn regression_fallback_rechecks_embargo_extended_during_html_request() {
    let mock = Mock::start().await;
    let n = mock.notifier();
    let sender = Arc::clone(&n);
    mock.set(move |_, count| {
        if count == 1 {
            // Deterministic equivalent of another sender extending the shared embargo
            // while this request is outstanding. No production Telegram traffic.
            sender.record_rate_limit(2);
            Reply::Json(
                400,
                json!({"ok":false,"description":"mock HTML parse error"}),
            )
        } else {
            Reply::Json(200, json!({"ok":true}))
        }
    });
    n.dispatch_alert_event(&event(&n, "small message"))
        .await
        .unwrap();
    let requests = mock.captured();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].at.duration_since(requests[0].at) >= Duration::from_millis(1800),
        "fallback started before the shared embargo expired"
    );
}

#[tokio::test]
async fn control_fallback_429_uses_actual_two_seconds_not_default_five() {
    let mock = Mock::start().await;
    mock.set(|body, _| {
        if body.get("parse_mode").is_some() {
            Reply::Json(
                400,
                json!({"ok":false,"description":"mock HTML parse error"}),
            )
        } else {
            Reply::Json(429, json!({"ok":false,"parameters":{"retry_after":2}}))
        }
    });
    let n = mock.notifier();
    assert!(n
        .dispatch_alert_event(&event(&n, "rate test"))
        .await
        .is_err());
    let remaining = n
        .rate_limit_remaining_duration()
        .expect("embargo after final 429");
    assert!(
        remaining <= Duration::from_millis(2100),
        "parsing must not silently select the 5s fallback"
    );
    assert!(
        remaining >= Duration::from_secs(1),
        "expected a recently established 2s embargo"
    );
    let requests = mock.captured();
    assert_eq!(requests.len(), 6);
    for i in [2, 4] {
        assert!(requests[i].at.duration_since(requests[i - 1].at) >= Duration::from_millis(1800));
    }
}

#[test]
fn control_prune_limits_actual_records_and_protects_pending_and_inflight() {
    let key = DeduplicationKey {
        source: AlertSource::LocalRule,
        target_id: Some("node-a".into()),
        reason_code: AlertReason::ServiceCheckFailed,
    };
    let mut records = HashMap::new();
    for generation in (0..=50).chain(std::iter::once(100)) {
        records.insert(
            generation,
            GenerationRecord {
                delivered_cooldown: Some((AlertSeverity::Warning, Instant::now())),
                ..GenerationRecord::default()
            },
        );
    }
    let mut state = DeliveryState::default();
    state.incidents.insert(
        key.clone(),
        IncidentState {
            current_generation: 100,
            records,
        },
    );
    let pending = AlertEvent {
        source: key.source,
        target_id: key.target_id.clone(),
        reason_code: key.reason_code,
        generation: 0,
        severity: AlertSeverity::Warning,
        title: "history".into(),
        message: "history".into(),
        timestamp: chrono::Utc::now(),
    };
    state.pending.insert(
        IncidentId {
            key: key.clone(),
            generation: 0,
        },
        pending,
    );
    state.in_flight.insert(IncidentId {
        key: key.clone(),
        generation: 1,
    });
    state.prune_incident_records();
    let records = &state.incidents[&key].records;
    assert!(records.contains_key(&0));
    assert!(records.contains_key(&1));
    assert!(records.contains_key(&100));
    assert_eq!(records.len(), 35); // 32 unprotected + current + pending + in-flight.
    assert_eq!(
        records
            .keys()
            .filter(|g| **g != 0 && **g != 1 && **g != 100)
            .count(),
        32
    );
}
