use crate::config::SelfHealingSettings;
use crate::core::models::SentinelDecision;
use crate::error::Result;
use tracing::{info, warn};

pub struct SelfHealingManager {
    settings: SelfHealingSettings,
}

impl SelfHealingManager {
    pub fn new(settings: SelfHealingSettings) -> Self {
        Self { settings }
    }

    pub async fn evaluate_and_heal(&self, decision: &SentinelDecision) -> Result<()> {
        if !self.settings.enabled {
            return Ok(());
        }

        if !decision.action_required {
            return Ok(());
        }

        info!(
            "Self-healing trigger activated by Jev decision: action='{}', risk={:.2}",
            decision.suggested_action, decision.risk_score
        );

        match decision.suggested_action.as_str() {
            "restart_unhealthy_service" => {
                warn!("Automated service restart requested by Jev System 1 (audit log recorded)");
                // In production, this can invoke allowlisted systemctl or launchctl kickstart commands
            }
            "purge_cache" => {
                info!("Automated cache purge requested by Jev System 1");
            }
            _ => {
                info!("No automated handler for action '{}'", decision.suggested_action);
            }
        }

        Ok(())
    }
}
