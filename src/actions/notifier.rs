use crate::config::TelegramAlertSettings;
use crate::core::models::{InfrastructureSnapshot, SentinelDecision, TargetStatus};
use crate::error::{Result, SentinelError};
use reqwest::{Client, Proxy};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tracing::{debug, error, info};

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

pub struct Notifier {
    client: Client,
    telegram: Option<TelegramAlertSettings>,
    delivered_events: Mutex<HashMap<DeduplicationKey, (AlertSeverity, std::time::Instant)>>,
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
            delivered_events: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.telegram.is_some()
    }

    pub async fn dispatch_alert_event(&self, event: &AlertEvent) -> Result<bool> {
        let Some(tg) = &self.telegram else {
            return Ok(false);
        };

        let min_sev = tg.min_severity.to_lowercase();
        let should_alert = match min_sev.as_str() {
            "info" => true,
            "warning" => event.severity >= AlertSeverity::Warning,
            "critical" => event.severity >= AlertSeverity::Critical,
            _ => event.severity >= AlertSeverity::Warning,
        };

        if !should_alert {
            return Ok(false);
        }

        let key = DeduplicationKey {
            source: event.source,
            target_id: event.target_id.clone(),
            reason_code: event.reason_code,
        };

        // Check deduplication: suppress if same or lower severity within 300s cooldown
        {
            let delivered = self.delivered_events.lock().unwrap();
            if let Some((prev_sev, sent_at)) = delivered.get(&key) {
                if event.severity <= *prev_sev && sent_at.elapsed() < Duration::from_secs(300) {
                    debug!("Suppressed duplicate alert for {:?}", key);
                    return Ok(false);
                }
            }
        }

        let formatted = self.format_alert_event(event);
        self.send_raw_message(&formatted).await?;

        // Record delivery only after Telegram confirms success
        {
            let mut delivered = self.delivered_events.lock().unwrap();
            delivered.insert(key, (event.severity, std::time::Instant::now()));
        }

        Ok(true)
    }

    pub fn clear_alert_cooldown(&self, target_id: &str) {
        if let Ok(mut delivered) = self.delivered_events.lock() {
            delivered.retain(|k, _| k.target_id.as_deref() != Some(target_id));
        }
    }

    pub fn clear_advisor_cooldown(&self) {
        if let Ok(mut delivered) = self.delivered_events.lock() {
            delivered.retain(|k, _| k.reason_code != AlertReason::AdvisorUnavailable);
        }
    }

    pub fn format_alert_event(&self, event: &AlertEvent) -> String {
        let icon = match event.severity {
            AlertSeverity::Info => "ℹ️",
            AlertSeverity::Warning => "⚠️",
            AlertSeverity::Critical => "🚨",
        };
        let safe_title = escape_html(&truncate_field(&event.title, 80));
        let mut lines = vec![format!("<b>{} Jev Sentinel: {}</b>", icon, safe_title)];
        if let Some(ref target) = event.target_id {
            lines.push(format!(
                "<b>Target:</b> <code>{}</code>",
                escape_html(&truncate_field(target, 80))
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
            escape_html(&decision.system_health.to_uppercase())
        ));
        lines.push(format!(
            "<b>Risk score:</b> {:.2} / 1.00",
            decision.risk_score
        ));
        lines.push(format!(
            "<b>Suggested Action:</b> <code>{}</code>",
            escape_html(&decision.suggested_action)
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
                escape_html(&target.target_name),
                escape_html(&target.target_type),
                target.latency_ms
            ));
            if let Some(err) = &target.error_message {
                let single_line_err = err.replace(['\r', '\n'], " ");
                let truncated_err = truncate_field(&single_line_err, 180);
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

        for chunk in chunks {
            let body = json!({
                "chat_id": tg.chat_id,
                "text": chunk,
                "parse_mode": "HTML",
                "disable_web_page_preview": true
            });

            let resp = self
                .client
                .post(&tg_url)
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    SentinelError::Action(format!("Network error sending Telegram message: {}", e))
                })?;

            let status = resp.status();
            if status.as_u16() == 400 {
                // Fallback to plain-text send if HTML parse error
                let fallback_body = json!({
                    "chat_id": tg.chat_id,
                    "text": truncate_field(&chunk, 4096),
                    "disable_web_page_preview": true
                });
                let fb_resp = self
                    .client
                    .post(&tg_url)
                    .json(&fallback_body)
                    .send()
                    .await
                    .map_err(|e| {
                        SentinelError::Action(format!("Network error in Telegram fallback: {}", e))
                    })?;
                Self::check_telegram_response(fb_resp).await?;
            } else {
                Self::check_telegram_response(resp).await?;
            }
        }

        info!("Dispatched Telegram notification to chat '{}'", tg.chat_id);
        Ok(())
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
    fn test_deduplication_lifecycle_and_advisor_recovery() {
        let notifier = Notifier::new(Some(TelegramAlertSettings {
            bot_token: "fake-token".to_string(),
            chat_id: "123456".to_string(),
            min_severity: "warning".to_string(),
            proxy: None,
        }));

        let event = AlertEvent {
            source: AlertSource::LocalRule,
            target_id: Some("server-1".to_string()),
            severity: AlertSeverity::Warning,
            reason_code: AlertReason::ServiceDegraded,
            title: "Service Degraded".to_string(),
            message: "High latency detected".to_string(),
            timestamp: chrono::Utc::now(),
        };

        let key = DeduplicationKey {
            source: event.source,
            target_id: event.target_id.clone(),
            reason_code: event.reason_code,
        };

        // 1. Manually insert delivery record
        {
            let mut delivered = notifier.delivered_events.lock().unwrap();
            delivered.insert(
                key.clone(),
                (AlertSeverity::Warning, std::time::Instant::now()),
            );
        }

        // 2. Same severity within cooldown -> suppressed
        {
            let delivered = notifier.delivered_events.lock().unwrap();
            let (prev_sev, sent_at) = delivered.get(&key).unwrap();
            assert!(event.severity <= *prev_sev && sent_at.elapsed() < Duration::from_secs(300));
        }

        // 3. Escalation to Critical -> not suppressed
        let crit_event = AlertEvent {
            severity: AlertSeverity::Critical,
            ..event.clone()
        };
        {
            let delivered = notifier.delivered_events.lock().unwrap();
            let (prev_sev, _) = delivered.get(&key).unwrap();
            assert!(!(crit_event.severity <= *prev_sev));
        }

        // 4. Target recovery clears cooldown
        notifier.clear_alert_cooldown("server-1");
        {
            let delivered = notifier.delivered_events.lock().unwrap();
            assert!(!delivered.contains_key(&key));
        }

        // 5. Advisor recovery clears advisor cooldown
        let adv_key = DeduplicationKey {
            source: AlertSource::LocalRule,
            target_id: None,
            reason_code: AlertReason::AdvisorUnavailable,
        };
        {
            let mut delivered = notifier.delivered_events.lock().unwrap();
            delivered.insert(
                adv_key.clone(),
                (AlertSeverity::Warning, std::time::Instant::now()),
            );
        }
        notifier.clear_advisor_cooldown();
        {
            let delivered = notifier.delivered_events.lock().unwrap();
            assert!(!delivered.contains_key(&adv_key));
        }
    }
}
