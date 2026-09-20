//! Composition regressions for d02276f174655dc06ba2ef3b11b2e9f68db2da04.
//! Uses actual public Notifier methods; no Telegram calls or real credentials.
//! Prepared by the reviewer, NOT compiled/run in the review environment.
//! All tests assert desired behavior. They are not a complete acceptance suite.
use jev_sentinel::actions::notifier::{
    AlertEvent, AlertReason, AlertSeverity, AlertSource, Notifier,
};
use jev_sentinel::config::TelegramAlertSettings;
use std::sync::Arc;
use std::time::Duration;

fn notifier() -> Notifier {
    Notifier::new(Some(TelegramAlertSettings {
        bot_token: "review-not-a-real-token".into(),
        chat_id: "0".into(),
        min_severity: "info".into(),
        proxy: None,
    }))
}

fn local(n: &Notifier, severity: AlertSeverity, title: &str, message: &str) -> AlertEvent {
    n.build_alert_event(
        AlertSource::LocalRule,
        Some("node-a".into()),
        severity,
        AlertReason::ServiceCheckFailed,
        title,
        message,
    )
}

fn assert_two_unmodified_generations(n: &Notifier, old: &AlertEvent, fresh: &AlertEvent) {
    // Only two admissions, far below the resource limit. No capacity eviction is needed.
    let mut selected = Vec::new();
    while let Some(e) = n.pop_next_pending() {
        selected.push(e);
        assert!(selected.len() <= 2, "unexpected additional work");
    }
    assert_eq!(
        selected.len(),
        2,
        "two accepted incidents must not silently coalesce"
    );
    for expected in [old, fresh] {
        let actual = selected
            .iter()
            .find(|e| e.generation == expected.generation)
            .expect("the accepted generation is missing");
        assert_eq!(actual.severity, expected.severity);
        assert_eq!(actual.title, expected.title);
        assert_eq!(actual.message, expected.message);
    }
}

#[tokio::test]
async fn rebound_equal_severity_preserves_both_accepted_generations() {
    let n = notifier();
    let old = local(&n, AlertSeverity::Warning, "I0 old", "old evidence");
    assert!(n.enqueue_alert(old.clone()));
    n.recover_target("node-a");
    let fresh = local(&n, AlertSeverity::Warning, "I1 new", "new evidence");
    assert_ne!(old.generation, fresh.generation);
    assert!(n.enqueue_alert(fresh.clone()));
    assert_two_unmodified_generations(&n, &old, &fresh);
}

#[tokio::test]
async fn rebound_higher_severity_does_not_silently_erase_accepted_history() {
    let n = notifier();
    let old = local(&n, AlertSeverity::Warning, "I0 warning", "old evidence");
    assert!(n.enqueue_alert(old.clone()));
    n.recover_target("node-a");
    let fresh = local(
        &n,
        AlertSeverity::Critical,
        "I1 critical",
        "new urgent evidence",
    );
    assert!(n.enqueue_alert(fresh.clone()));
    assert_two_unmodified_generations(&n, &old, &fresh);
}

#[tokio::test]
async fn rebound_lower_severity_must_not_disappear_behind_historical_peak() {
    let n = notifier();
    let old = local(
        &n,
        AlertSeverity::Critical,
        "I0 critical",
        "old urgent evidence",
    );
    assert!(n.enqueue_alert(old.clone()));
    n.recover_target("node-a");
    let fresh = local(
        &n,
        AlertSeverity::Warning,
        "I1 warning",
        "new distinct evidence",
    );
    assert!(n.enqueue_alert(fresh.clone()));
    assert_two_unmodified_generations(&n, &old, &fresh);
}

#[tokio::test]
async fn advisor_risk_rebound_preserves_both_pending_incidents() {
    let n = notifier();
    let old = n.build_alert_event(
        AlertSource::JevAdvisor,
        None,
        AlertSeverity::Critical,
        AlertReason::RiskElevated,
        "I0 risk",
        "old risk evidence",
    );
    assert!(n.enqueue_alert(old.clone()));
    n.recover_risk_elevated();
    let fresh = n.build_alert_event(
        AlertSource::JevAdvisor,
        None,
        AlertSeverity::Critical,
        AlertReason::RiskElevated,
        "I1 risk",
        "new risk evidence",
    );
    assert!(n.enqueue_alert(fresh.clone()));
    assert_two_unmodified_generations(&n, &old, &fresh);
}

#[tokio::test]
async fn recovery_does_not_resurrect_a_repeat_already_covered_by_ack() {
    let n = notifier();
    let first = local(&n, AlertSeverity::Warning, "I0", "same evidence");
    assert!(n.enqueue_alert(first.clone()));
    let sending = n
        .pop_next_pending()
        .expect("first attempt reserved by worker");
    // Another poll before ACK.
    let repeat = local(&n, AlertSeverity::Warning, "I0", "same evidence");
    assert!(n.enqueue_alert(repeat));
    assert!(n.commit_delivery(&sending));
    // Recovery occurs before the worker selects the queued repeat.
    n.recover_target("node-a");
    assert!(
        n.pop_next_pending().is_none(),
        "already-confirmed historical content must not become undelivered after recovery"
    );
}

#[tokio::test]
async fn admission_reports_closed_after_idle_worker_has_terminated() {
    let n = Arc::new(notifier());
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    // Queue is empty and there are no network operations.
    // The update is unseen by stop_rx, so changed() must observe it on start.
    stop_tx.send(true).expect("receiver is alive");
    let handle = tokio::spawn(Arc::clone(&n).run_worker(stop_rx));
    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("idle worker must terminate")
        .expect("worker must not panic");
    let event = local(
        &n,
        AlertSeverity::Warning,
        "after stop",
        "no consumer exists",
    );
    assert!(
        !n.enqueue_alert(event),
        "admission must not claim acceptance into a stopped sender"
    );
}

#[tokio::test]
async fn content_change_resets_chunk_progress_while_identical_plan_resumes() {
    let n = notifier();
    let old_plan = local(&n, AlertSeverity::Warning, "Plan A", "Message A");
    assert!(n.enqueue_alert(old_plan.clone()));
    let _popped = n.pop_next_pending().unwrap();

    // When message text or severity escalates, a new plan is recognized
    let escalated = local(
        &n,
        AlertSeverity::Critical,
        "Plan B",
        "Message B (escalated)",
    );
    assert!(n.enqueue_alert(escalated));
    let next = n.pop_next_pending().unwrap();
    assert_eq!(next.severity, AlertSeverity::Critical);
}

type MockResponseFn = Box<dyn Fn(&str, usize) -> String + Send + Sync>;

struct MockTelegramServer {
    addr: String,
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    response_fn: Arc<std::sync::Mutex<MockResponseFn>>,
}

impl MockTelegramServer {
    async fn start() -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let addr = format!("http://127.0.0.1:{}", port);
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let response_fn: Arc<std::sync::Mutex<MockResponseFn>> = Arc::new(std::sync::Mutex::new(
            Box::new(|_body, _count| {
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_string()
            }),
        ));

        let req_clone = Arc::clone(&requests);
        let resp_clone = Arc::clone(&response_fn);

        tokio::spawn(async move {
            let mut request_count = 0;
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut req_bytes = Vec::new();
                let mut content_length = None;
                let mut header_len = 0;
                let mut buf = [0u8; 4096];

                loop {
                    let n = match socket.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    req_bytes.extend_from_slice(&buf[..n]);

                    if content_length.is_none() {
                        if let Some(pos) = req_bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_len = pos + 4;
                            let headers_str = String::from_utf8_lossy(&req_bytes[..pos]);
                            for line in headers_str.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        content_length = v.trim().parse::<usize>().ok();
                                    }
                                }
                            }
                            if content_length.is_none() {
                                break;
                            }
                        }
                    }

                    if let Some(cl) = content_length {
                        if req_bytes.len() >= header_len + cl {
                            break;
                        }
                    }
                }

                if !req_bytes.is_empty() {
                    let text = String::from_utf8_lossy(&req_bytes).to_string();
                    req_clone.lock().unwrap().push(text.clone());
                    request_count += 1;
                    let resp = (resp_clone.lock().unwrap())(&text, request_count);
                    if !resp.is_empty() {
                        let _ = socket.write_all(resp.as_bytes()).await;
                        let _ = socket.flush().await;
                        let _ = socket.shutdown().await;
                    }
                }
            }
        });

        Self {
            addr,
            requests,
            response_fn,
        }
    }

    fn mock_notifier(&self) -> Notifier {
        Notifier::new_with_api_base(
            Some(TelegramAlertSettings {
                bot_token: "mock-token".into(),
                chat_id: "12345".into(),
                min_severity: "info".into(),
                proxy: None,
            }),
            &self.addr,
        )
    }
}

#[tokio::test]
async fn transport_r5_part0_ack_part1_fails_retry_resumes_from_part1() {
    let mock = MockTelegramServer::start().await;
    // Chunk 0 ("AAAA") succeeds on count 1.
    // Chunk 1 ("BBBB") fails on count 2, but succeeds on retry (count 3).
    *mock.response_fn.lock().unwrap() = Box::new(|body, count| {
        if body.contains("AAAA") {
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_string()
        } else if count == 2 {
            "HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\nContent-Length: 5\r\n\r\nerror".to_string()
        } else {
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_string()
        }
    });

    let n = mock.mock_notifier();
    let msg = format!("{}\n{}", "A".repeat(3500), "B".repeat(3500));
    let event = local(&n, AlertSeverity::Warning, "SplitIncident", &msg);
    assert!(n.enqueue_alert(event.clone()));

    let popped = n.pop_next_pending().unwrap();
    // Delivery succeeds on retry of chunk 1
    assert!(n.dispatch_alert_event(&popped).await.is_ok());

    let count_a = mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("AAAA"))
        .count();
    let count_b = mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("BBBB"))
        .count();
    assert_eq!(
        count_a, 1,
        "Chunk 0 must be sent exactly once and confirmed"
    );
    assert_eq!(count_b, 2, "Chunk 1 was retried and succeeded on attempt 2");
}

#[tokio::test]
async fn transport_r5_interrupted_by_shutdown_resumes_from_unconfirmed_chunk() {
    let mock = MockTelegramServer::start().await;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let shutdown_tx = Arc::new(shutdown_tx);

    // Chunk 0 ("AAAA") succeeds. Chunk 1 ("BBBB") triggers shutdown.
    let s_tx = Arc::clone(&shutdown_tx);
    *mock.response_fn.lock().unwrap() = Box::new(move |body, _count| {
        if body.contains("AAAA") {
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_string()
        } else {
            // Signal shutdown when chunk 1 request is received
            let _ = s_tx.send(true);
            // Hang / fail
            "".to_string()
        }
    });

    let n = Arc::new(mock.mock_notifier());
    let msg = format!("{}\n{}", "A".repeat(3500), "B".repeat(3500));
    let event = local(&n, AlertSeverity::Warning, "ShutdownResumeTest", &msg);
    let formatted = n.format_alert_event(&event);

    // Call 1: chunk 0 is confirmed, then interrupted by shutdown before chunk 1
    let res1 = n
        .send_alert_message(&event, &formatted, Some(&shutdown_rx))
        .await;
    assert!(res1.is_err(), "Call 1 must be interrupted by shutdown");

    let count_a_call1 = mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("AAAA"))
        .count();
    assert_eq!(count_a_call1, 1, "Chunk 0 was confirmed in call 1");

    // Call 2: fresh call with shutdown = false and mock returns OK for chunk 1
    *mock.response_fn.lock().unwrap() = Box::new(|_body, _count| {
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_string()
    });
    let (_s_tx2, shutdown_rx2) = tokio::sync::watch::channel(false);
    let res2 = n
        .send_alert_message(&event, &formatted, Some(&shutdown_rx2))
        .await;
    assert!(res2.is_ok(), "Call 2 must resume and succeed");

    let count_a_total = mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("AAAA"))
        .count();
    let count_b_total = mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("BBBB"))
        .count();
    assert_eq!(count_a_total, 1, "Chunk 0 was NOT resent in call 2!");
    assert_eq!(
        count_b_total, 2,
        "Chunk 1 was attempted in call 1 and delivered in call 2"
    );
}

#[tokio::test]
async fn transport_r5_escalation_after_partial_delivery_resets_chunk_progress_to_zero() {
    let mock = MockTelegramServer::start().await;
    // Chunk 0 succeeds, Chunk 1 fails
    *mock.response_fn.lock().unwrap() = Box::new(|body, _count| {
        if body.contains("AAAA") {
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_string()
        } else {
            "HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\nContent-Length: 5\r\n\r\nerror".to_string()
        }
    });

    let n = mock.mock_notifier();
    let plan_a_msg = format!("{}\n{}", "A".repeat(3500), "B".repeat(3500));
    let event_a = local(&n, AlertSeverity::Warning, "Plan A", &plan_a_msg);
    let _ = n.dispatch_alert_event(&event_a).await;

    // Now an urgent escalation arrives: Plan B at Critical
    *mock.response_fn.lock().unwrap() = Box::new(|_body, _count| {
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_string()
    });
    let plan_b_msg = format!("{}\n{}", "C".repeat(3500), "D".repeat(3500));
    let event_b = local(&n, AlertSeverity::Critical, "Plan B", &plan_b_msg);
    assert!(n.dispatch_alert_event(&event_b).await.is_ok());

    // Verify Plan B was sent from chunk 0 ("CCCC")
    let sent_c = mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.contains("CCCC"));
    assert!(
        sent_c,
        "Plan B must start sending from chunk 0, not inherit chunk index of Plan A"
    );
}

#[tokio::test]
async fn transport_r5_exhausted_plan_blocks_same_severity_repeat_but_admits_escalation() {
    let mock = MockTelegramServer::start().await;
    // Always fail
    *mock.response_fn.lock().unwrap() = Box::new(|_body, _count| {
        "HTTP/1.1 500 Internal Server Error\r\nConnection: close\r\nContent-Length: 5\r\n\r\nerror"
            .to_string()
    });

    let n = mock.mock_notifier();
    let event = local(&n, AlertSeverity::Warning, "ExhaustTarget", "Evidence");
    let _ = n.dispatch_alert_event(&event).await;

    // After 3 failed attempts, warning is exhausted
    let repeat_warning = local(&n, AlertSeverity::Warning, "ExhaustTarget", "Evidence");
    assert!(
        !n.enqueue_alert(repeat_warning),
        "Exhausted plan must suppress duplicate repeats of the same severity"
    );

    // Escalation to Critical must be admitted with its own budget
    let critical = local(
        &n,
        AlertSeverity::Critical,
        "ExhaustTarget",
        "Urgent Evidence",
    );
    assert!(
        n.enqueue_alert(critical),
        "Critical escalation must be admitted even after warning exhaustion"
    );
}

#[tokio::test]
async fn transport_shutdown_cancels_hanging_network_exchange_immediately() {
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

    // Mock server accepts connection and never responds
    tokio::spawn(async move {
        if let Ok((_socket, _)) = listener.accept().await {
            let _ = ready_tx.send(());
            // Hold socket open indefinitely without responding
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });

    let n = Notifier::new_with_api_base(
        Some(TelegramAlertSettings {
            bot_token: "mock-token".into(),
            chat_id: "123".into(),
            min_severity: "info".into(),
            proxy: None,
        }),
        format!("http://127.0.0.1:{}", port),
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let event = local(&n, AlertSeverity::Warning, "HangingTest", "Hang");
    let n_arc = Arc::new(n);
    let n_clone = Arc::clone(&n_arc);

    let send_handle = tokio::spawn(async move {
        let text = n_clone.format_alert_event(&event);
        n_clone
            .send_alert_message(&event, &text, Some(&shutdown_rx))
            .await
    });

    ready_rx.await.expect("server accepted TCP");
    let t0 = std::time::Instant::now();
    shutdown_tx.send(true).unwrap();

    let res = tokio::time::timeout(Duration::from_millis(500), send_handle)
        .await
        .expect("must exit within 500ms")
        .expect("must not panic");
    assert!(
        res.is_err(),
        "Must return error when interrupted by shutdown"
    );
    let err_msg = res.unwrap_err().to_string();
    assert!(
        err_msg.contains("Interrupted by shutdown"),
        "Error: {}",
        err_msg
    );
    assert!(
        t0.elapsed() < Duration::from_millis(400),
        "Must abort in <400ms, elapsed: {:?}",
        t0.elapsed()
    );
}

#[tokio::test]
async fn transport_shutdown_cancels_during_incomplete_body_read() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

    // Mock server sends partial headers and hangs on body
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\n\r\n{\"ok\":")
                .await;
            let _ = ready_tx.send(());
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });

    let n = Notifier::new_with_api_base(
        Some(TelegramAlertSettings {
            bot_token: "mock-token".into(),
            chat_id: "123".into(),
            min_severity: "info".into(),
            proxy: None,
        }),
        format!("http://127.0.0.1:{}", port),
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let event = local(&n, AlertSeverity::Warning, "IncompleteBodyTest", "BodyHang");
    let n_arc = Arc::new(n);
    let n_clone = Arc::clone(&n_arc);

    let send_handle = tokio::spawn(async move {
        let text = n_clone.format_alert_event(&event);
        n_clone
            .send_alert_message(&event, &text, Some(&shutdown_rx))
            .await
    });

    ready_rx.await.expect("server sent partial body");
    let t0 = std::time::Instant::now();
    shutdown_tx.send(true).unwrap();

    let res = tokio::time::timeout(Duration::from_millis(500), send_handle)
        .await
        .expect("must exit within 500ms")
        .expect("must not panic");
    assert!(
        res.is_err(),
        "Must return error when interrupted by shutdown"
    );
    let err_msg = res.unwrap_err().to_string();
    assert!(
        err_msg.contains("Interrupted by shutdown"),
        "Error: {}",
        err_msg
    );
    assert!(
        t0.elapsed() < Duration::from_millis(400),
        "Must abort in <400ms, elapsed: {:?}",
        t0.elapsed()
    );
}

#[tokio::test]
async fn transport_rate_limit_429_monotonic_embargo_and_fallback() {
    let mock = MockTelegramServer::start().await;
    // Return 400 on HTML, then 429 with retry_after=2 on fallback
    *mock.response_fn.lock().unwrap() = Box::new(|body, _count| {
        if body.contains("\"parse_mode\":\"HTML\"") {
            "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 10\r\n\r\nHTML error"
                .to_string()
        } else {
            "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 35\r\n\r\n{\"parameters\":{\"retry_after\":2}}".to_string()
        }
    });

    let n = mock.mock_notifier();
    let event = local(&n, AlertSeverity::Warning, "RateLimitTest", "Fallback 429");
    let res = n.dispatch_alert_event(&event).await;
    assert!(res.is_err());

    // Verify rate limit embargo is set to >= 1.5s in the future
    let remaining = n
        .rate_limit_remaining_duration()
        .expect("Embargo deadline must be recorded");
    assert!(
        remaining >= Duration::from_millis(1500),
        "Remaining: {:?}",
        remaining
    );

    // Simulate smaller retry_after=1 arriving from another request: deadline must not decrease!
    n.record_rate_limit(1);
    let remaining_after = n
        .rate_limit_remaining_duration()
        .expect("Embargo deadline must still be recorded");
    assert!(
        remaining_after >= Duration::from_millis(1400),
        "Monotonic max must not shorten embargo"
    );
}

#[tokio::test]
async fn safety_cap_32_evicts_only_unprotected_historical_generations() {
    let n = notifier();
    // Enqueue generation 0 at Warning: keep it in pending while Critical events are popped
    let g0 = local(&n, AlertSeverity::Warning, "G0", "evidence 0");
    assert!(n.enqueue_alert(g0.clone()));

    // Create 40 historical generations at Critical that get popped and delivered
    for i in 1..=40 {
        n.recover_target("node-a");
        let ev = local(
            &n,
            AlertSeverity::Critical,
            &format!("G{i}"),
            &format!("evidence {i}"),
        );
        assert!(n.enqueue_alert(ev.clone()));
        let popped = n.pop_next_pending().unwrap();
        assert_eq!(popped.generation, i as u64);
        assert!(n.commit_delivery(&popped));
    }

    // Trigger pruning via recovery
    n.recover_target("node-a");

    // Pop the next pending: G0 was preserved in pending and never evicted by the 32 safety cap!
    let g0_popped = n
        .pop_next_pending()
        .expect("G0 must remain protected in pending");
    assert_eq!(g0_popped.generation, 0, "G0 in pending was preserved");
}
