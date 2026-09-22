use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetStatus {
    Online,
    Degraded,
    Unreachable,
    Timeout,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetTelemetry {
    pub target_name: str_or_string::TargetName,
    pub target_type: String,
    pub status: TargetStatus,
    pub latency_ms: f64,
    pub metrics: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub timestamp: DateTime<Utc>,
    #[serde(skip)]
    pub observed_at: Option<std::time::Instant>,
}

mod str_or_string {
    pub type TargetName = String;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfrastructureSnapshot {
    pub timestamp: DateTime<Utc>,
    pub targets: Vec<TargetTelemetry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetAnomaly {
    pub name: String,
    pub target_type: String,
    pub status: TargetStatus,
    pub latency_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetLoadSummary {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_used_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_used_mib: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyDigest {
    pub timestamp: DateTime<Utc>,
    pub total_targets: usize,
    pub online_targets: usize,
    pub summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub anomalies: Vec<TargetAnomaly>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub elevated_loads: Vec<TargetLoadSummary>,
}

impl InfrastructureSnapshot {
    pub fn has_active_anomalies(&self) -> bool {
        self.targets.iter().any(|t| {
            if t.status != TargetStatus::Online || t.error_message.is_some() {
                return true;
            }
            let m = crate::core::history::NormalizedMetrics::from_telemetry(t);
            let high_cpu = m.cpu_pct.is_some_and(|c| c > 80.0);
            let high_ram = m.ram_used_pct.is_some_and(|r| r > 85.0);
            high_cpu || high_ram
        })
    }

    pub fn build_digest(&self) -> AnomalyDigest {
        let total = self.targets.len();
        let mut online = 0;
        let mut anomalies = Vec::new();
        let mut elevated_loads = Vec::new();

        for t in &self.targets {
            if t.status == TargetStatus::Online && t.error_message.is_none() {
                online += 1;
            } else {
                anomalies.push(TargetAnomaly {
                    name: t.target_name.clone(),
                    target_type: t.target_type.clone(),
                    status: t.status.clone(),
                    latency_ms: (t.latency_ms * 10.0).round() / 10.0,
                    error_message: t.error_message.clone(),
                });
            }

            let m = crate::core::history::NormalizedMetrics::from_telemetry(t);
            let high_cpu = m.cpu_pct.is_some_and(|c| c > 70.0);
            let high_ram = m.ram_used_pct.is_some_and(|r| r > 85.0);
            let high_swap = m.swap_used_mib.is_some_and(|s| s > 500.0);

            if high_cpu || high_ram || high_swap {
                elevated_loads.push(TargetLoadSummary {
                    name: t.target_name.clone(),
                    cpu_pct: m.cpu_pct.map(|c| (c * 10.0).round() / 10.0),
                    ram_used_pct: m.ram_used_pct.map(|r| (r * 10.0).round() / 10.0),
                    swap_used_mib: m.swap_used_mib.map(|s| s.round()),
                });
            }
        }

        let summary = format!("{online} of {total} targets online and healthy");
        AnomalyDigest {
            timestamp: self.timestamp,
            total_targets: total,
            online_targets: online,
            summary,
            anomalies,
            elevated_loads,
        }
    }
}

// ── TypeSafe Jev Request Models ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemOneRequest {
    pub model: String,
    pub state: String,
    pub questions: HashMap<String, QuestionSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QuestionSpec {
    Noul {
        instructions: String,
        criteria: HashMap<String, String>,
    },
    Choice {
        instructions: String,
        criteria: HashMap<String, String>,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

// ── TypeSafe Jev Response Models ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemOneResponse {
    pub model: String,
    pub answers: HashMap<String, JevAnswer>,
    #[serde(default)]
    pub usage: Option<JevUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JevUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JevAnswer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        #[serde(default)]
        confidence: Option<f64>,
        #[serde(default)]
        probabilities: Option<HashMap<String, f64>>,
    },
    Score {
        score: f64,
        #[serde(default)]
        confidence: Option<f64>,
    },
}

// ── High-Level Decision ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentinelDecision {
    pub timestamp: DateTime<Utc>,
    pub system_health: String, // "healthy", "degraded", "critical"
    pub health_confidence: Option<f64>,
    pub risk_score: f64,       // 0.0 .. 1.0
    pub action_required: bool, // derived from noul
    pub action_probability: f64,
    pub suggested_action: String, // "none", "restart_service", "alert_admin", etc.
    pub raw_answers: HashMap<String, JevAnswer>,
}

impl SentinelDecision {
    pub fn is_critical(&self) -> bool {
        self.system_health == "critical" || (self.risk_score >= 0.85 && self.action_required)
    }

    pub fn is_warning(&self) -> bool {
        self.system_health == "degraded" || self.risk_score >= 0.40
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decision_severity_flags() {
        let nominal = SentinelDecision {
            timestamp: Utc::now(),
            system_health: "healthy".to_string(),
            health_confidence: Some(0.98),
            risk_score: 0.10,
            action_required: false,
            action_probability: 0.05,
            suggested_action: "none".to_string(),
            raw_answers: HashMap::new(),
        };
        assert!(!nominal.is_warning());
        assert!(!nominal.is_critical());

        let warning = SentinelDecision {
            timestamp: Utc::now(),
            system_health: "degraded".to_string(),
            health_confidence: Some(0.85),
            risk_score: 0.45,
            action_required: false,
            action_probability: 0.30,
            suggested_action: "none".to_string(),
            raw_answers: HashMap::new(),
        };
        assert!(warning.is_warning());
        assert!(!warning.is_critical());

        let critical = SentinelDecision {
            timestamp: Utc::now(),
            system_health: "critical".to_string(),
            health_confidence: Some(0.99),
            risk_score: 0.95,
            action_required: true,
            action_probability: 0.99,
            suggested_action: "restart_unhealthy_service".to_string(),
            raw_answers: HashMap::new(),
        };
        assert!(critical.is_warning());
        assert!(critical.is_critical());
    }

    #[test]
    fn test_snapshot_anomaly_detection_and_digest() {
        let now = Utc::now();
        let target_healthy = TargetTelemetry {
            target_name: "healthy-node".to_string(),
            target_type: "http_probe".to_string(),
            status: TargetStatus::Online,
            latency_ms: 2.5,
            metrics: serde_json::json!({"status_code": 200}),
            error_message: None,
            timestamp: now,
            observed_at: None,
        };

        let snapshot_healthy = InfrastructureSnapshot {
            timestamp: now,
            targets: vec![target_healthy.clone()],
        };

        assert!(!snapshot_healthy.has_active_anomalies());
        let digest_healthy = snapshot_healthy.build_digest();
        assert_eq!(digest_healthy.total_targets, 1);
        assert_eq!(digest_healthy.online_targets, 1);
        assert!(digest_healthy.anomalies.is_empty());

        let target_failing = TargetTelemetry {
            target_name: "failing-node".to_string(),
            target_type: "http_probe".to_string(),
            status: TargetStatus::Unreachable,
            latency_ms: 0.0,
            metrics: serde_json::json!({"status_code": 500}),
            error_message: Some("Connection refused".to_string()),
            timestamp: now,
            observed_at: None,
        };

        let snapshot_failing = InfrastructureSnapshot {
            timestamp: now,
            targets: vec![target_healthy, target_failing],
        };

        assert!(snapshot_failing.has_active_anomalies());
        let digest_failing = snapshot_failing.build_digest();
        assert_eq!(digest_failing.total_targets, 2);
        assert_eq!(digest_failing.online_targets, 1);
        assert_eq!(digest_failing.anomalies.len(), 1);
        assert_eq!(digest_failing.anomalies[0].name, "failing-node");
    }
}
