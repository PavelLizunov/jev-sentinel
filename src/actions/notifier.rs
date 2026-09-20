use crate::config::TelegramAlertSettings;
use crate::core::models::{InfrastructureSnapshot, SentinelDecision, TargetStatus};
use crate::error::{Result, SentinelError};
use reqwest::{Client, Proxy};
use serde_json::json;
use std::time::Duration;
use tracing::{debug, error, info};

pub fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub struct Notifier {
    client: Client,
    telegram: Option<TelegramAlertSettings>,
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

        Self { client, telegram }
    }

    pub fn is_configured(&self) -> bool {
        self.telegram.is_some()
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
                let truncated_err = if err.len() > 200 {
                    format!("{}...", &err[..197])
                } else {
                    err.clone()
                };
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

        let safe_text = if message_text.len() > 4000 {
            format!("{}...\n<i>(message truncated)</i>", &message_text[..3950])
        } else {
            message_text.to_string()
        };

        let tg_url = format!("https://api.telegram.org/bot{}/sendMessage", tg.bot_token);
        let body = json!({
            "chat_id": tg.chat_id,
            "text": safe_text,
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
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            let desc = format!("Telegram API HTTP {}: {}", status, err_body);
            error!("{}", desc);
            return Err(SentinelError::Action(desc));
        }

        info!("Dispatched Telegram notification to chat '{}'", tg.chat_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
