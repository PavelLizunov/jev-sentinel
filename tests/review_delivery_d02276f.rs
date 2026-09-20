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
