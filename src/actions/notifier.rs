use crate::config::TelegramAlertSettings;
use crate::core::models::{InfrastructureSnapshot, SentinelDecision};
use crate::error::Result;
use reqwest::Client;
use serde_json::json;
use tracing::{error, info};

pub struct Notifier {
    client: Client,
    telegram: Option<TelegramAlertSettings>,
}

impl Notifier {
    pub fn new(telegram: Option<TelegramAlertSettings>) -> Self {
        Self {
            client: Client::new(),
            telegram,
        }
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
            return Ok(());
        }

        let icon = match decision.system_health.as_str() {
            "healthy" => "✅",
            "degraded" => "⚠️",
            "critical" => "🚨",
            _ => "ℹ️",
        };

        let mut lines = Vec::new();
        lines.push(format!("<b>{} Jev Sentinel Alert: {}</b>", icon, decision.system_health.to_uppercase()));
        lines.push(format!("<b>Health Score:</b> {:.2} (risk: {:.0}%)", decision.risk_score, decision.risk_score * 100.0));
        lines.push(format!("<b>Suggested Action:</b> <code>{}</code>", decision.suggested_action));
        lines.push(format!("<b>Confidence:</b> {:.0}%", decision.health_confidence * 100.0));
        lines.push("".to_string());
        lines.push("<b>Target Telemetry Summary:</b>".to_string());

        for target in &snapshot.targets {
            let status_emoji = match target.status {
                crate::core::models::TargetStatus::Online => "🟢",
                crate::core::models::TargetStatus::Degraded => "🟡",
                crate::core::models::TargetStatus::Unreachable => "🔴",
                crate::core::models::TargetStatus::Timeout => "⏱️",
            };
            lines.push(format!(
                "• {} <b>{}</b> ({}): {:.1}ms",
                status_emoji, target.target_name, target.target_type, target.latency_ms
            ));
            if let Some(err) = &target.error_message {
                lines.push(format!("  <i>Error:</i> <code>{}</code>", err));
            }
        }

        let message_text = lines.join("\n");
        let tg_url = format!("https://api.telegram.org/bot{}/sendMessage", tg.bot_token);

        let body = json!({
            "chat_id": tg.chat_id,
            "text": message_text,
            "parse_mode": "HTML",
            "disable_web_page_preview": true
        });

        match self.client.post(&tg_url).json(&body).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!("Dispatched Telegram alert for system health '{}'", decision.system_health);
                } else {
                    let err = resp.text().await.unwrap_or_default();
                    error!("Failed to dispatch Telegram alert: {}", err);
                }
            }
            Err(e) => {
                error!("Error sending Telegram alert: {}", e);
            }
        }

        Ok(())
    }
}
