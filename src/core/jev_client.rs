use crate::core::models::*;
use crate::error::{Result, SentinelError};
use chrono::Utc;
use reqwest::{Client, Proxy};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info};

pub struct JevClient {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl JevClient {
    pub fn new(
        api_key: String,
        model: Option<String>,
        base_url: Option<String>,
        egress_proxy: Option<String>,
        timeout_seconds: Option<u64>,
    ) -> Result<Self> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(timeout_seconds.unwrap_or(15)));

        if let Some(proxy_url) = egress_proxy {
            if !proxy_url.trim().is_empty() {
                debug!("Configuring JevClient with egress proxy: {}", proxy_url);
                let proxy = Proxy::all(&proxy_url)
                    .map_err(|e| SentinelError::Config(format!("Invalid proxy URL '{}': {}", proxy_url, e)))?;
                builder = builder.proxy(proxy);
            }
        }

        let client = builder.build()?;
        let model = model.unwrap_or_else(|| "jev-latest".to_string());
        let base_url = base_url
            .unwrap_or_else(|| "https://api.typesafe.ai".to_string())
            .trim_end_matches('/')
            .to_string();

        Ok(Self {
            client,
            api_key,
            base_url,
            model,
        })
    }

    pub async fn evaluate_snapshot(&self, snapshot: &InfrastructureSnapshot) -> Result<SentinelDecision> {
        let state_json = serde_json::to_string_pretty(snapshot)?;
        debug!("Evaluating snapshot state:\n{}", state_json);

        let mut questions = HashMap::new();

        // 1. Overall System Health Choice
        let mut health_criteria = HashMap::new();
        health_criteria.insert(
            "healthy".to_string(),
            "All targets are online and reporting normal parameters without resource saturation or failures".to_string(),
        );
        health_criteria.insert(
            "degraded".to_string(),
            "One or more auxiliary services are unreachable, memory/swap pressure is elevated, or non-critical warnings detected".to_string(),
        );
        health_criteria.insert(
            "critical".to_string(),
            "Primary node is offline, memory exhaustion is imminent, or critical cluster service has failed".to_string(),
        );
        questions.insert(
            "system_health".to_string(),
            QuestionSpec::Choice {
                instructions: "Evaluate the overall operational health of the monitored infrastructure based on the multi-node telemetry snapshot.".to_string(),
                criteria: health_criteria,
            },
        );

        // 2. Risk Score (0.0 .. 1.0)
        questions.insert(
            "risk_score".to_string(),
            QuestionSpec::Score {
                instructions: "Rate the immediate operational risk level of system failure, kernel panic, or outage from 0.0 (safe) to 1.0 (imminent failure).".to_string(),
                criteria: vec![
                    "Nominal - System running safely within parameters".to_string(),
                    "Moderate - Elevated memory, swap growth, or latency detected".to_string(),
                    "Critical - Urgent risk of crash, OOM panic, or service outage".to_string(),
                ],
            },
        );

        // 3. Action Required Noul
        let mut action_criteria = HashMap::new();
        action_criteria.insert(
            "true".to_string(),
            "Automated self-healing or urgent administrator intervention is necessary to prevent system degradation or crash".to_string(),
        );
        action_criteria.insert(
            "false".to_string(),
            "System is operating within safe tolerances; no intervention needed at this time".to_string(),
        );
        questions.insert(
            "action_required".to_string(),
            QuestionSpec::Noul {
                instructions: "Is immediate automated or operator intervention required based on current trends and errors?".to_string(),
                criteria: action_criteria,
            },
        );

        // 4. Suggested Action Choice
        let mut action_choice_criteria = HashMap::new();
        action_choice_criteria.insert(
            "none".to_string(),
            "No action required; continue passive monitoring".to_string(),
        );
        action_choice_criteria.insert(
            "restart_unhealthy_service".to_string(),
            "Restart a specific crashed, frozen, or runaway service/container to restore health".to_string(),
        );
        action_choice_criteria.insert(
            "purge_cache".to_string(),
            "Clean caches or release unneeded allocations to avert swap exhaustion".to_string(),
        );
        action_choice_criteria.insert(
            "alert_operator".to_string(),
            "Trigger high-priority alert to operator for manual investigation or physical access".to_string(),
        );
        questions.insert(
            "suggested_action".to_string(),
            QuestionSpec::Choice {
                instructions: "Determine the most appropriate immediate action to maintain cluster stability.".to_string(),
                criteria: action_choice_criteria,
            },
        );

        let request_payload = SystemOneRequest {
            model: self.model.clone(),
            state: state_json,
            questions,
        };

        let url = format!("{}/v1/systemone", self.base_url);
        info!("Dispatching snapshot evaluation to TypeSafe Jev at {}", url);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&request_payload)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            return Err(SentinelError::JevApi(format!(
                "HTTP {}: {}",
                status, error_text
            )));
        }

        let jev_resp: SystemOneResponse = resp.json().await?;
        debug!("TypeSafe Jev response parsed successfully: {:?}", jev_resp);

        // Extract answers
        let system_health = match jev_resp.answers.get("system_health") {
            Some(JevAnswer::Choice { choice, .. }) => choice.clone(),
            _ => "unknown".to_string(),
        };

        let health_confidence = match jev_resp.answers.get("system_health") {
            Some(JevAnswer::Choice { confidence, .. }) => confidence.unwrap_or(0.5),
            _ => 0.5,
        };

        let risk_score = match jev_resp.answers.get("risk_score") {
            Some(JevAnswer::Score { score, .. }) => *score,
            _ => 0.0,
        };

        let (action_required, action_probability) = match jev_resp.answers.get("action_required") {
            Some(JevAnswer::Noul { noul }) => (*noul >= 0.50, *noul),
            _ => (false, 0.0),
        };

        let suggested_action = match jev_resp.answers.get("suggested_action") {
            Some(JevAnswer::Choice { choice, .. }) => choice.clone(),
            _ => "none".to_string(),
        };

        Ok(SentinelDecision {
            timestamp: Utc::now(),
            system_health,
            health_confidence,
            risk_score,
            action_required,
            action_probability,
            suggested_action,
            raw_answers: jev_resp.answers,
        })
    }
}
