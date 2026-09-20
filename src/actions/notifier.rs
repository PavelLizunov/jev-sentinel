use crate::config::TelegramAlertSettings;
use crate::core::models::{InfrastructureSnapshot, SentinelDecision, TargetStatus};
use crate::error::{Result, SentinelError};
use reqwest::{Client, Proxy};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tracing::{debug, error, info, warn};

pub fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn truncate_field(input: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let char_count = input.chars().count();
    if char_count <= max_chars {
        return input.to_string();
    }
    let budget = max_chars.saturating_sub(1);
    let prefix: String = input.chars().take(budget).collect();
    format!("{prefix}…")
}

pub fn sanitize_inline_field(input: &str, max_chars: usize) -> String {
    let single_line = input.replace(['\r', '\n'], " ");
    truncate_field(&single_line, max_chars)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertSource {
    LocalRule,
    JevAdvisor,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertReason {
    ObservationUnavailable,
    ServiceCheckFailed,
    ServiceDegraded,
    AdvisorUnavailable,
    RiskElevated,
}

#[derive(Debug, Clone)]
pub struct AlertEvent {
    pub source: AlertSource,
    pub target_id: Option<String>,
    pub severity: AlertSeverity,
    pub reason_code: AlertReason,
    pub generation: u64,
    pub title: String,
    pub message: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeduplicationKey {
    pub source: AlertSource,
    pub target_id: Option<String>,
    pub reason_code: AlertReason,
}

#[derive(Debug, Clone)]
pub struct IncidentState {
    pub current_generation: u64,
    pub delivered_cooldown: Option<(AlertSeverity, std::time::Instant)>,
}

#[derive(Default)]
pub struct DeliveryState {
    pub incidents: HashMap<DeduplicationKey, IncidentState>,
    pub pending: HashMap<DeduplicationKey, AlertEvent>,
}

pub struct Notifier {
    client: Client,
    telegram: Option<TelegramAlertSettings>,
    delivery_state: Mutex<DeliveryState>,
    notify: tokio::sync::Notify,
}

impl Notifier {
    pub fn new(telegram: Option<TelegramAlertSettings>) -> Self {
        Self::new_with_fallback(telegram, None)
    }

    pub fn new_with_fallback(
        telegram: Option<TelegramAlertSettings>,
        fallback_proxy: Option<&str>,
    ) -> Self {
        let client = if let Some(ref tg) = telegram {
            let mut builder = Client::builder().timeout(Duration::from_secs(10));

            let proxy_candidate = tg
                .proxy
                .as_deref()
                .or(fallback_proxy)
                .map(|s| s.to_string())
                .or_else(|| std::env::var("TELEGRAM_PROXY").ok())
                .or_else(|| std::env::var("HTTPS_PROXY").ok())
                .or_else(|| std::env::var("https_proxy").ok())
                .or_else(|| std::env::var("ALL_PROXY").ok())
                .or_else(|| std::env::var("all_proxy").ok());

            if let Some(ref proxy_url) = proxy_candidate {
                let trimmed = proxy_url.trim();
                if !trimmed.is_empty() {
                    match Proxy::all(trimmed) {
                        Ok(proxy) => {
                            debug!("Configured Telegram notifier proxy: {}", trimmed);
                            builder = builder.proxy(proxy);
                        }
                        Err(e) => {
                            error!("Invalid Telegram proxy URL '{}': {}", trimmed, e);
                        }
                    }
                }
            }

            builder.build().unwrap_or_else(|_| Client::new())
        } else {
            Client::new()
        };

        Self {
            client,
            telegram,
            delivery_state: Mutex::new(DeliveryState::default()),
            notify: tokio::sync::Notify::new(),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.telegram.is_some()
    }

    pub fn build_alert_event(
        &self,
        source: AlertSource,
        target_id: Option<String>,
        severity: AlertSeverity,
        reason_code: AlertReason,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> AlertEvent {
        let title = title.into();
        let message = message.into();
        let key = DeduplicationKey {
            source,
            target_id: target_id.clone(),
            reason_code,
        };
        let generation = {
            let mut state = self.delivery_state.lock().unwrap();
            let inc = state.incidents.entry(key).or_insert_with(|| IncidentState {
                current_generation: 0,
                delivered_cooldown: None,
            });
            inc.current_generation
        };

        AlertEvent {
            source,
            target_id,
            severity,
            reason_code,
            generation,
            title,
            message,
            timestamp: chrono::Utc::now(),
        }
    }

    pub fn enqueue_alert(&self, event: AlertEvent) -> bool {
        let Some(tg) = &self.telegram else {
            return false;
        };

        let min_sev = tg.min_severity.to_lowercase();
        let should_alert = match min_sev.as_str() {
            "info" => true,
            "warning" => event.severity >= AlertSeverity::Warning,
            "critical" => event.severity >= AlertSeverity::Critical,
            _ => event.severity >= AlertSeverity::Warning,
        };

        if !should_alert {
            return false;
        }

        let key = DeduplicationKey {
            source: event.source,
            target_id: event.target_id.clone(),
            reason_code: event.reason_code,
        };

        let mut state = self.delivery_state.lock().unwrap();
        let incident = state
            .incidents
            .entry(key.clone())
            .or_insert_with(|| IncidentState {
                current_generation: event.generation,
                delivered_cooldown: None,
            });

        // Discard obsolete event if target has already recovered/advanced generation
        if event.generation < incident.current_generation {
            debug!(
                "Discarding obsolete event for {:?} (event gen {}, current gen {})",
                key, event.generation, incident.current_generation
            );
            return false;
        }

        // Suppress duplicate if delivered recently with same or higher severity
        if let Some((prev_sev, sent_at)) = incident.delivered_cooldown {
            if event.severity <= prev_sev && sent_at.elapsed() < Duration::from_secs(300) {
                debug!("Suppressed duplicate alert for {:?}", key);
                return false;
            }
        }

        // Coalesce into bounded pending table (max 64 entries)
        if let Some(existing) = state.pending.get_mut(&key) {
            if event.severity > existing.severity {
                *existing = event; // Escalate severity!
            } else {
                existing.timestamp = event.timestamp;
                existing.message = event.message;
            }
        } else {
            if state.pending.len() >= 64 {
                // Priority admission: evict lower severity if available
                if let Some(evict_key) = state
                    .pending
                    .iter()
                    .filter(|(_, e)| e.severity < event.severity)
                    .map(|(k, _)| k.clone())
                    .next()
                {
                    state.pending.remove(&evict_key);
                    state.pending.insert(key, event);
                } else {
                    warn!(
                        "Pending alert table full (64), dropping event for {:?}",
                        key
                    );
                    return false;
                }
            } else {
                state.pending.insert(key, event);
            }
        }

        self.notify.notify_one();
        true
    }

    pub fn pop_next_pending(&self) -> Option<AlertEvent> {
        let mut state = self.delivery_state.lock().unwrap();
        if state.pending.is_empty() {
            return None;
        }

        // Pick highest severity event (Critical -> Warning -> Info)
        let key_to_pop = state
            .pending
            .iter()
            .max_by_key(|(_, e)| e.severity)
            .map(|(k, _)| k.clone())?;

        state.pending.remove(&key_to_pop)
    }

    pub fn commit_delivery(&self, event: &AlertEvent) -> bool {
        let key = DeduplicationKey {
            source: event.source,
            target_id: event.target_id.clone(),
            reason_code: event.reason_code,
        };

        let mut state = self.delivery_state.lock().unwrap();
        if let Some(inc) = state.incidents.get_mut(&key) {
            if inc.current_generation == event.generation {
                inc.delivered_cooldown = Some((event.severity, std::time::Instant::now()));
                debug!(
                    "Committed delivery for {:?} at generation {}",
                    key, event.generation
                );
                return true;
            } else {
                debug!(
                    "Discarded late Telegram ACK for {:?}: generation changed from {} to {}",
                    key, event.generation, inc.current_generation
                );
            }
        }
        false
    }

    pub fn recover_target(&self, target_id: &str) {
        let mut state = self.delivery_state.lock().unwrap();
        for (k, inc) in state.incidents.iter_mut() {
            if k.target_id.as_deref() == Some(target_id) {
                inc.current_generation = inc.current_generation.wrapping_add(1);
                inc.delivered_cooldown = None;
            }
        }
        state
            .pending
            .retain(|k, _| k.target_id.as_deref() != Some(target_id));
    }

    pub fn recover_advisor_unavailable(&self) {
        let mut state = self.delivery_state.lock().unwrap();
        let adv_key = DeduplicationKey {
            source: AlertSource::LocalRule,
            target_id: None,
            reason_code: AlertReason::AdvisorUnavailable,
        };
        if let Some(inc) = state.incidents.get_mut(&adv_key) {
            inc.current_generation = inc.current_generation.wrapping_add(1);
            inc.delivered_cooldown = None;
        }
        state.pending.remove(&adv_key);
    }

    pub fn recover_risk_elevated(&self) {
        let mut state = self.delivery_state.lock().unwrap();
        let risk_key = DeduplicationKey {
            source: AlertSource::JevAdvisor,
            target_id: None,
            reason_code: AlertReason::RiskElevated,
        };
        if let Some(inc) = state.incidents.get_mut(&risk_key) {
            inc.current_generation = inc.current_generation.wrapping_add(1);
            inc.delivered_cooldown = None;
        }
        state.pending.remove(&risk_key);
    }

    pub async fn run_worker(
        self: std::sync::Arc<Self>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                _ = self.notify.notified() => {
                    while let Some(event) = self.pop_next_pending() {
                        let formatted = self.format_alert_event(&event);
                        if let Err(e) = self.send_raw_message(&formatted).await {
                            error!("Alert delivery failed after retries for {:?}: {}", event.target_id, e);
                        } else {
                            self.commit_delivery(&event);
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
    }

    pub fn format_alert_event(&self, event: &AlertEvent) -> String {
        let icon = match event.severity {
            AlertSeverity::Info => "ℹ️",
            AlertSeverity::Warning => "⚠️",
            AlertSeverity::Critical => "🚨",
        };
        let safe_title = escape_html(&sanitize_inline_field(&event.title, 80));
        let mut lines = vec![format!("<b>{} Jev Sentinel: {}</b>", icon, safe_title)];
        if let Some(ref target) = event.target_id {
            lines.push(format!(
                "<b>Target:</b> <code>{}</code>",
                escape_html(&sanitize_inline_field(target, 80))
            ));
        }
        lines.push(format!("<b>Severity:</b> {:?}", event.severity));
        lines.push(format!("<b>Reason:</b> {:?}", event.reason_code));
        lines.push(format!(
            "<b>Timestamp:</b> {}",
            event.timestamp.to_rfc3339()
        ));
        lines.push("".to_string());
        lines.push(escape_html(&truncate_field(&event.message, 1000)));
        lines.join("\n")
    }

    pub async fn dispatch_alert_event(&self, event: &AlertEvent) -> Result<bool> {
        if !self.enqueue_alert(event.clone()) {
            return Ok(false);
        }
        let formatted = self.format_alert_event(event);
        self.send_raw_message(&formatted).await?;
        Ok(self.commit_delivery(event))
    }

    pub async fn dispatch_decision(
        &self,
        decision: &SentinelDecision,
        snapshot: &InfrastructureSnapshot,
    ) -> Result<()> {
        let Some(tg) = &self.telegram else {
            return Ok(());
        };

        let min_sev = tg.min_severity.to_lowercase();
        let should_alert = match min_sev.as_str() {
            "info" => true,
            "warning" => decision.is_warning() || decision.is_critical(),
            "critical" => decision.is_critical(),
            _ => decision.is_warning() || decision.is_critical(),
        };

        if !should_alert {
            debug!(
                "Suppressed Telegram alert for health '{}' (min_severity='{}')",
                decision.system_health, tg.min_severity
            );
            return Ok(());
        }

        self.dispatch_decision_forced(decision, snapshot).await
    }

    pub async fn dispatch_decision_forced(
        &self,
        decision: &SentinelDecision,
        snapshot: &InfrastructureSnapshot,
    ) -> Result<()> {
        let message_text = self.format_decision_message(decision, snapshot);
        self.send_raw_message(&message_text).await
    }

    pub async fn send_test_alert(&self) -> Result<()> {
        let Some(tg) = &self.telegram else {
            return Err(SentinelError::Config(
                "Telegram alerting is not configured in sentinel.yaml".to_string(),
            ));
        };

        let host = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .unwrap_or_else(|_| "localhost".to_string());

        let lines = [
            "🚀 <b>Jev Sentinel: Telegram Alert Test</b>".to_string(),
            "".to_string(),
            format!("<b>Host:</b> <code>{}</code>", escape_html(&host)),
            format!(
                "<b>Chat ID:</b> <code>{}</code>",
                escape_html(&tg.chat_id)
            ),
            format!(
                "<b>Timestamp:</b> <code>{}</code>",
                chrono::Utc::now().to_rfc3339()
            ),
            "<b>Status:</b> ✅ Channel verified and active.".to_string(),
            "".to_string(),
            "<i>This is a test notification dispatched by <code>jev-sentinel test-alert</code>.</i>"
                .to_string(),
        ];

        let message_text = lines.join("\n");
        self.send_raw_message(&message_text).await
    }

    fn format_decision_message(
        &self,
        decision: &SentinelDecision,
        snapshot: &InfrastructureSnapshot,
    ) -> String {
        let icon = match decision.system_health.as_str() {
            "healthy" => "✅",
            "degraded" => "⚠️",
            "critical" => "🚨",
            _ => "ℹ️",
        };

        let mut lines = Vec::new();
        lines.push(format!(
            "<b>{} Jev Sentinel Alert: {}</b>",
            icon,
            escape_html(&sanitize_inline_field(
                &decision.system_health.to_uppercase(),
                40
            ))
        ));
        lines.push(format!(
            "<b>Risk score:</b> {:.2} / 1.00",
            decision.risk_score
        ));
        lines.push(format!(
            "<b>Suggested Action:</b> <code>{}</code>",
            escape_html(&sanitize_inline_field(&decision.suggested_action, 80))
        ));
        if let Some(confidence) = decision.health_confidence {
            lines.push(format!("<b>Confidence:</b> {:.0}%", confidence * 100.0));
        }
        lines.push("".to_string());
        lines.push("<b>Target Telemetry Summary:</b>".to_string());

        for target in &snapshot.targets {
            let status_emoji = match target.status {
                TargetStatus::Online => "🟢",
                TargetStatus::Degraded => "🟡",
                TargetStatus::Unreachable => "🔴",
                TargetStatus::Timeout => "⏱️",
                TargetStatus::Unknown => "❔",
            };
            lines.push(format!(
                "• {} <b>{}</b> ({}): {:.1}ms",
                status_emoji,
                escape_html(&sanitize_inline_field(&target.target_name, 60)),
                escape_html(&sanitize_inline_field(&target.target_type, 30)),
                target.latency_ms
            ));
            if let Some(err) = &target.error_message {
                let truncated_err = sanitize_inline_field(err, 180);
                lines.push(format!(
                    "  <i>Error:</i> <code>{}</code>",
                    escape_html(&truncated_err)
                ));
            }
        }

        lines.join("\n")
    }

    async fn send_raw_message(&self, message_text: &str) -> Result<()> {
        let Some(tg) = &self.telegram else {
            return Ok(());
        };

        let tg_url = format!("https://api.telegram.org/bot{}/sendMessage", tg.bot_token);

        // Group lines into chunks bounded to 3900 characters without splitting HTML tags
        let mut chunks = Vec::new();
        let mut current_chunk = String::new();
        for line in message_text.lines() {
            if current_chunk.chars().count() + line.chars().count() + 1 > 3900
                && !current_chunk.is_empty()
            {
                chunks.push(current_chunk);
                current_chunk = String::new();
            }
            if !current_chunk.is_empty() {
                current_chunk.push('\n');
            }
            current_chunk.push_str(line);
        }
        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }

        let mut confirmed_chunks = 0usize;
        let mut last_err = None;

        for attempt in 0..3 {
            let mut send_err = None;
            while confirmed_chunks < chunks.len() {
                let chunk = &chunks[confirmed_chunks];
                let body = json!({
                    "chat_id": tg.chat_id,
                    "text": chunk,
                    "parse_mode": "HTML",
                    "disable_web_page_preview": true
                });

                let send_res = self.client.post(&tg_url).json(&body).send().await;
                let resp = match send_res {
                    Ok(r) => r,
                    Err(e) => {
                        send_err = Some(SentinelError::Action(format!(
                            "Network error sending Telegram message: {}",
                            e
                        )));
                        break;
                    }
                };

                let status = resp.status();
                let check_res = if status.as_u16() == 400 {
                    let fallback_body = json!({
                        "chat_id": tg.chat_id,
                        "text": truncate_field(chunk, 4096),
                        "disable_web_page_preview": true
                    });
                    match self.client.post(&tg_url).json(&fallback_body).send().await {
                        Ok(fb_resp) => Self::check_telegram_response(fb_resp).await,
                        Err(e) => Err(SentinelError::Action(format!(
                            "Network error in Telegram fallback: {}",
                            e
                        ))),
                    }
                } else {
                    Self::check_telegram_response(resp).await
                };

                match check_res {
                    Ok(()) => {
                        confirmed_chunks += 1;
                    }
                    Err(e) => {
                        send_err = Some(e);
                        break;
                    }
                }
            }

            if confirmed_chunks == chunks.len() {
                info!("Dispatched Telegram notification to chat '{}'", tg.chat_id);
                return Ok(());
            }

            last_err = send_err;
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }

        Err(last_err.unwrap_or_else(|| {
            SentinelError::Action("Failed to deliver alert after retries".into())
        }))
    }

    async fn check_telegram_response(resp: reqwest::Response) -> Result<()> {
        let status = resp.status();
        if !status.is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            let desc = format!("Telegram API HTTP {}: {}", status, err_text);
            error!("{}", desc);
            return Err(SentinelError::Action(desc));
        }

        let resp_json: serde_json::Value = resp.json().await.map_err(|e| {
            SentinelError::Action(format!("Telegram API invalid JSON response: {}", e))
        })?;

        if resp_json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let desc = resp_json
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("Telegram API response ok=false");
            return Err(SentinelError::Action(format!(
                "Telegram API error: {}",
                desc
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::TargetTelemetry;
    use std::collections::HashMap;

    #[test]
    fn test_escape_html_entities() {
        let raw = "Service <critical> & timeout > 5000ms";
        let escaped = escape_html(raw);
        assert_eq!(
            escaped,
            "Service &lt;critical&gt; &amp; timeout &gt; 5000ms"
        );
    }

    #[test]
    fn test_format_decision_message() {
        let notifier = Notifier::new(None);
        let decision = SentinelDecision {
            timestamp: chrono::Utc::now(),
            system_health: "degraded".to_string(),
            health_confidence: Some(0.92),
            risk_score: 0.45,
            action_required: true,
            action_probability: 0.88,
            suggested_action: "restart_unhealthy_service".to_string(),
            raw_answers: HashMap::new(),
        };

        let snapshot = InfrastructureSnapshot {
            timestamp: chrono::Utc::now(),
            targets: vec![],
        };

        let msg = notifier.format_decision_message(&decision, &snapshot);
        assert!(msg.contains("⚠️ Jev Sentinel Alert: DEGRADED"));
        assert!(msg.contains("restart_unhealthy_service"));
    }

    #[test]
    fn test_truncate_field_boundary_cases() {
        assert_eq!(truncate_field("текст", 0), "");
        assert_eq!(truncate_field("текст", 1), "…");
        assert_eq!(truncate_field("", 0), "");
        assert_eq!(truncate_field("я", 1), "я");
        assert_eq!(truncate_field("длинный русский текст", 10), "длинный р…");
        assert!(truncate_field("длинный русский текст", 10).chars().count() <= 10);
    }

    #[test]
    fn test_truncate_field_cyrillic_and_emojis() {
        // String of 300 Cyrillic characters with emojis
        let long_error =
            "🚨 Ошибка подключения к базе данных: сервер перегружен или недоступен! ".repeat(5);
        let truncated = truncate_field(&long_error, 180);
        assert!(truncated.chars().count() <= 180);
        assert!(truncated.ends_with('…'));
        // Verify HTML escaping on truncated text works without panic or entity split
        let escaped = escape_html(&truncated);
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('>'));
    }

    #[test]
    fn test_multiline_error_remains_single_line_in_html() {
        let notifier = Notifier::new(None);
        let decision = SentinelDecision {
            timestamp: chrono::Utc::now(),
            system_health: "degraded".to_string(),
            health_confidence: Some(0.92),
            risk_score: 0.45,
            action_required: true,
            action_probability: 0.88,
            suggested_action: "none".to_string(),
            raw_answers: HashMap::new(),
        };

        let multiline_err = "x\n".to_string() + &"y".repeat(80);
        let snapshot = InfrastructureSnapshot {
            timestamp: chrono::Utc::now(),
            targets: vec![TargetTelemetry {
                target_name: "test-node".to_string(),
                target_type: "exec_probe".to_string(),
                status: TargetStatus::Degraded,
                latency_ms: 5.0,
                metrics: serde_json::json!({}),
                error_message: Some(multiline_err),
                timestamp: chrono::Utc::now(),
                observed_at: None,
            }],
        };

        let msg = notifier.format_decision_message(&decision, &snapshot);
        for line in msg.lines() {
            if line.contains("<code>") {
                assert!(
                    line.contains("</code>"),
                    "<code> tag must close on the same line: {}",
                    line
                );
            }
        }
    }

    #[test]
    fn test_multiline_target_names_remain_single_line_in_html() {
        let notifier = Notifier::new(None);
        let decision = SentinelDecision {
            timestamp: chrono::Utc::now(),
            system_health: "degraded".to_string(),
            health_confidence: Some(0.92),
            risk_score: 0.45,
            action_required: true,
            action_probability: 0.88,
            suggested_action: "none".to_string(),
            raw_answers: HashMap::new(),
        };

        // 29 targets with names containing newlines: "node-i\nzzz"
        let targets = (0..29)
            .map(|i| TargetTelemetry {
                target_name: format!("node-{i}\nzzz"),
                target_type: "exec_probe".to_string(),
                status: TargetStatus::Degraded,
                latency_ms: 1.0,
                metrics: serde_json::json!({}),
                error_message: Some("x\n".to_string() + &"y".repeat(80)),
                timestamp: chrono::Utc::now(),
                observed_at: None,
            })
            .collect();

        let snapshot = InfrastructureSnapshot {
            timestamp: chrono::Utc::now(),
            targets,
        };

        let msg = notifier.format_decision_message(&decision, &snapshot);
        for line in msg.lines() {
            if line.contains("<b>") {
                assert!(
                    line.contains("</b>"),
                    "<b> tag must close on the same line: {}",
                    line
                );
            }
            if line.contains("<code>") {
                assert!(
                    line.contains("</code>"),
                    "<code> tag must close on the same line: {}",
                    line
                );
            }
        }
    }

    #[test]
    fn test_deduplication_lifecycle_and_advisor_recovery() {
        let notifier = Notifier::new(Some(TelegramAlertSettings {
            bot_token: "fake-token".to_string(),
            chat_id: "123456".to_string(),
            min_severity: "warning".to_string(),
            proxy: None,
        }));

        let event = notifier.build_alert_event(
            AlertSource::LocalRule,
            Some("server-1".to_string()),
            AlertSeverity::Warning,
            AlertReason::ServiceDegraded,
            "Service Degraded".to_string(),
            "High latency detected".to_string(),
        );

        // 1. Initial enqueue succeeds
        assert!(notifier.enqueue_alert(event.clone()));

        // 2. Commit delivery
        assert!(notifier.commit_delivery(&event));

        // 3. Same severity within cooldown -> suppressed
        assert!(!notifier.enqueue_alert(event.clone()));

        // 4. Escalation to Critical -> not suppressed
        let crit_event = AlertEvent {
            severity: AlertSeverity::Critical,
            ..event.clone()
        };
        assert!(notifier.enqueue_alert(crit_event));

        // 5. Target recovery clears cooldown and cancels pending
        notifier.recover_target("server-1");
        // Next event has new generation and is admitted immediately
        let new_event = notifier.build_alert_event(
            AlertSource::LocalRule,
            Some("server-1".to_string()),
            AlertSeverity::Warning,
            AlertReason::ServiceDegraded,
            "Service Degraded".to_string(),
            "High latency detected".to_string(),
        );
        assert!(notifier.enqueue_alert(new_event));

        // 6. Advisor recovery clears both advisor unavailable AND risk elevated cooldown
        let adv_event = notifier.build_alert_event(
            AlertSource::LocalRule,
            None,
            AlertSeverity::Warning,
            AlertReason::AdvisorUnavailable,
            "Advisor Unavailable".to_string(),
            "Network timeout".to_string(),
        );
        assert!(notifier.enqueue_alert(adv_event.clone()));
        notifier.commit_delivery(&adv_event);
        assert!(!notifier.enqueue_alert(adv_event.clone())); // suppressed
        notifier.recover_advisor_unavailable();
        let fresh_adv = notifier.build_alert_event(
            AlertSource::LocalRule,
            None,
            AlertSeverity::Warning,
            AlertReason::AdvisorUnavailable,
            "Advisor Unavailable".to_string(),
            "Network timeout".to_string(),
        );
        assert!(notifier.enqueue_alert(fresh_adv)); // allowed after recovery
    }

    #[test]
    fn test_late_ack_race_does_not_suppress_new_incident() {
        let notifier = Notifier::new(Some(TelegramAlertSettings {
            bot_token: "fake-token".to_string(),
            chat_id: "123456".to_string(),
            min_severity: "warning".to_string(),
            proxy: None,
        }));

        let target_id = "node-A";

        // t0: Incident 1 begins. Generation is stamped onto event at creation
        let i1 = notifier.build_alert_event(
            AlertSource::LocalRule,
            Some(target_id.to_string()),
            AlertSeverity::Warning,
            AlertReason::ServiceCheckFailed,
            "Service Check Failed".to_string(),
            "Error 500".to_string(),
        );
        assert_eq!(i1.generation, 0);

        // t1: Target recovers while HTTP is in flight!
        notifier.recover_target(target_id);

        // t2: Late HTTP response for Incident 1 arrives.
        // commit_delivery sees that target's generation is now 1, discarding late ACK
        assert!(!notifier.commit_delivery(&i1));

        // t3: Target fails again (Incident 2)!
        let i2 = notifier.build_alert_event(
            AlertSource::LocalRule,
            Some(target_id.to_string()),
            AlertSeverity::Warning,
            AlertReason::ServiceCheckFailed,
            "Service Check Failed".to_string(),
            "Error 500".to_string(),
        );
        assert_eq!(i2.generation, 1);

        // Incident 2 must NOT be suppressed!
        assert!(
            notifier.enqueue_alert(i2),
            "Incident 2 must not be suppressed by late ACK of Incident 1"
        );
    }

    #[test]
    fn test_pending_coalescing_and_priority_admission() {
        let notifier = Notifier::new(Some(TelegramAlertSettings {
            bot_token: "fake-token".to_string(),
            chat_id: "123456".to_string(),
            min_severity: "warning".to_string(),
            proxy: None,
        }));

        let warn_event = notifier.build_alert_event(
            AlertSource::LocalRule,
            Some("node-A".to_string()),
            AlertSeverity::Warning,
            AlertReason::ServiceDegraded,
            "Degraded".to_string(),
            "Load high".to_string(),
        );

        // Enqueue 128 duplicates of the same event: they must coalesce into 1 pending entry!
        for _ in 0..128 {
            assert!(notifier.enqueue_alert(warn_event.clone()));
        }

        {
            let state = notifier.delivery_state.lock().unwrap();
            assert_eq!(
                state.pending.len(),
                1,
                "Duplicates must coalesce into 1 pending slot"
            );
        }

        // A new Critical event arrives: must be admitted and prioritized over Warning!
        let crit_event = notifier.build_alert_event(
            AlertSource::JevAdvisor,
            None,
            AlertSeverity::Critical,
            AlertReason::RiskElevated,
            "Critical Outage".to_string(),
            "Urgent action required".to_string(),
        );
        assert!(notifier.enqueue_alert(crit_event));

        // Next popped pending event must be the Critical one!
        let popped = notifier
            .pop_next_pending()
            .expect("Must have pending event");
        assert_eq!(popped.severity, AlertSeverity::Critical);
    }

    #[test]
    fn test_risk_elevated_recovery_and_repeated_critical() {
        let notifier = Notifier::new(Some(TelegramAlertSettings {
            bot_token: "fake-token".to_string(),
            chat_id: "123456".to_string(),
            min_severity: "warning".to_string(),
            proxy: None,
        }));

        let crit = notifier.build_alert_event(
            AlertSource::JevAdvisor,
            None,
            AlertSeverity::Critical,
            AlertReason::RiskElevated,
            "Critical".to_string(),
            "OOM imminent".to_string(),
        );

        // 1. First critical alert is admitted and committed
        assert!(notifier.enqueue_alert(crit.clone()));
        assert!(notifier.commit_delivery(&crit));

        // 2. Continuing critical condition at t=60: must be suppressed within 300s cooldown!
        assert!(!notifier.enqueue_alert(crit.clone()));

        // 3. Jev reports healthy (risk normalized): recover_risk_elevated clears the cooldown
        notifier.recover_risk_elevated();

        // 4. A new critical risk arrives: must be admitted immediately!
        let crit_fresh = notifier.build_alert_event(
            AlertSource::JevAdvisor,
            None,
            AlertSeverity::Critical,
            AlertReason::RiskElevated,
            "Critical".to_string(),
            "OOM imminent".to_string(),
        );
        assert!(notifier.enqueue_alert(crit_fresh));
    }
}
