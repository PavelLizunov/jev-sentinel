//! State-contract regression tests for f60863ca2baad2e4c06ded76bcf158bd2629cfb9.
//! Calls actual public Notifier methods. No Telegram requests or production mutations.
//! Not compiled/run by the reviewer: cargo/rustc unavailable in the review runtime.
//! FAIL_* tests assert desired behavior, not acceptance of current bugs.
//! See README.md for anticipated results and policy qualifications.

use jev_sentinel::actions::notifier::{
    AlertEvent, AlertReason, AlertSeverity, AlertSource, Notifier,
};
use jev_sentinel::config::TelegramAlertSettings;

fn notifier() -> Notifier {
    Notifier::new(Some(TelegramAlertSettings {
        bot_token: "review-not-a-real-token".into(),
        chat_id: "0".into(),
        min_severity: "info".into(),
        proxy: None,
    }))
}

fn risk(n: &Notifier, severity: AlertSeverity, message: &str) -> AlertEvent {
    n.build_alert_event(
        AlertSource::JevAdvisor,
        None,
        severity,
        AlertReason::RiskElevated,
        format!("Risk {severity:?}"),
        message,
    )
}

fn local(n: &Notifier, target: &str, severity: AlertSeverity) -> AlertEvent {
    n.build_alert_event(
        AlertSource::LocalRule,
        Some(target.into()),
        severity,
        AlertReason::ServiceCheckFailed,
        "Service check failed",
        "A confirmed service-check error",
    )
}

#[tokio::test]
async fn pass_generation_stamped_before_queue_and_late_ack_rejected() {
    let n = notifier();
    let old = local(&n, "node-a", AlertSeverity::Warning);
    assert!(n.enqueue_alert(old.clone()));
    let in_flight = n.pop_next_pending().expect("old event");
    n.recover_target("node-a");
    let fresh = local(&n, "node-a", AlertSeverity::Warning);
    assert_ne!(old.generation, fresh.generation);
    assert!(!n.commit_delivery(&in_flight));
    assert!(n.enqueue_alert(fresh.clone()));
    let next = n.pop_next_pending().expect("new incident");
    assert_eq!(next.generation, fresh.generation);
}

#[tokio::test]
async fn pass_advisor_unavailable_has_its_own_generation() {
    let n = notifier();
    let old = n.build_alert_event(
        AlertSource::LocalRule,
        None,
        AlertSeverity::Warning,
        AlertReason::AdvisorUnavailable,
        "Advisor unavailable",
        "timeout",
    );
    assert!(n.enqueue_alert(old.clone()));
    let sent = n.pop_next_pending().expect("advisor event");
    n.recover_advisor_unavailable();
    assert!(!n.commit_delivery(&sent));
    let fresh = n.build_alert_event(
        AlertSource::LocalRule,
        None,
        AlertSeverity::Warning,
        AlertReason::AdvisorUnavailable,
        "Advisor unavailable",
        "timeout",
    );
    assert_ne!(old.generation, fresh.generation);
    assert!(n.enqueue_alert(fresh));
}

#[tokio::test]
async fn pass_successful_advisor_call_does_not_reset_risk_cooldown() {
    let n = notifier();
    let first = risk(&n, AlertSeverity::Critical, "risk elevated");
    assert!(n.enqueue_alert(first));
    let sent = n.pop_next_pending().expect("risk event");
    assert!(n.commit_delivery(&sent));
    n.recover_advisor_unavailable();
    let repeat = risk(&n, AlertSeverity::Critical, "risk still elevated");
    assert!(!n.enqueue_alert(repeat));
    n.recover_risk_elevated();
    let rebound = risk(&n, AlertSeverity::Critical, "new elevated-risk incident");
    assert!(n.enqueue_alert(rebound));
}

#[tokio::test]
async fn pass_full_64_unique_warnings_admits_critical_first() {
    let n = notifier();
    for i in 0..64 {
        let event = local(&n, &format!("node-{i}"), AlertSeverity::Warning);
        assert!(n.enqueue_alert(event));
    }
    let critical = risk(&n, AlertSeverity::Critical, "critical incident");
    assert!(n.enqueue_alert(critical));
    assert_eq!(
        n.pop_next_pending().expect("critical admitted").severity,
        AlertSeverity::Critical,
    );
    let mut remaining = 0;
    while n.pop_next_pending().is_some() {
        remaining += 1;
    }
    assert_eq!(remaining, 63);
}

#[tokio::test]
async fn fail_pending_repeat_must_not_be_selected_after_first_ack() {
    let n = notifier();
    let first = risk(&n, AlertSeverity::Critical, "same problem");
    assert!(n.enqueue_alert(first));
    let in_flight = n.pop_next_pending().expect("start first send");

    // Another polling cycle while the first HTTP request is outstanding.
    let repeat = risk(&n, AlertSeverity::Critical, "same problem");
    let _ = n.enqueue_alert(repeat); // coalescing/deferral policy may differ after the fix

    // The first request now receives a valid ACK.
    assert!(n.commit_delivery(&in_flight));
    assert!(
        n.pop_next_pending().is_none(),
        "worker must not send a redundant same-generation event after a valid ACK",
    );
}

#[tokio::test]
async fn pass_escalation_during_warning_send_must_not_be_discarded() {
    let n = notifier();
    assert!(n.enqueue_alert(risk(&n, AlertSeverity::Warning, "moderate risk")));
    let warning_in_flight = n.pop_next_pending().expect("warning send");
    assert!(n.enqueue_alert(risk(&n, AlertSeverity::Critical, "urgent risk")));
    assert!(n.commit_delivery(&warning_in_flight));
    let next = n
        .pop_next_pending()
        .expect("escalation must survive ACK cleanup");
    assert_eq!(next.severity, AlertSeverity::Critical);
    assert_eq!(next.message, "urgent risk");
}

#[tokio::test]
async fn fail_lower_severity_refresh_must_not_erase_critical_evidence() {
    let n = notifier();
    let critical = risk(
        &n,
        AlertSeverity::Critical,
        "critical evidence: OOM imminent",
    );
    let critical_title = critical.title.clone();
    assert!(n.enqueue_alert(critical));
    assert!(n.enqueue_alert(risk(
        &n,
        AlertSeverity::Warning,
        "current condition improved, but no recovery",
    )));
    let selected = n.pop_next_pending().expect("pending critical event");
    assert_eq!(selected.severity, AlertSeverity::Critical);
    assert_eq!(selected.title, critical_title);
    assert_eq!(
        selected.message, "critical evidence: OOM imminent",
        "keeping peak severity requires keeping its evidence, or explicitly separating peak/current",
    );
}

#[tokio::test]
async fn fail_recovery_must_not_silently_erase_an_accepted_undelivered_incident() {
    // Asserts the historical-notification policy described in the supplied delivery contract.
    // An explicitly agreed, observable cancellation policy could replace this expectation.
    let n = notifier();
    let old = local(&n, "brief-outage", AlertSeverity::Warning);
    assert!(n.enqueue_alert(old.clone()));
    n.recover_target("brief-outage");
    let historical = n.pop_next_pending().expect(
        "retain accepted historical alert, or expose an explicit terminal cancellation outcome",
    );
    assert_eq!(historical.generation, old.generation);
    // Historical delivery cannot start a current-generation cooldown.
    assert!(!n.commit_delivery(&historical));
}
