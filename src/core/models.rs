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
}

mod str_or_string {
    pub type TargetName = String;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfrastructureSnapshot {
    pub timestamp: DateTime<Utc>,
    pub targets: Vec<TargetTelemetry>,
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
    pub system_health: String,     // "healthy", "degraded", "critical"
    pub health_confidence: f64,
    pub risk_score: f64,           // 0.0 .. 1.0
    pub action_required: bool,     // derived from noul
    pub action_probability: f64,
    pub suggested_action: String,  // "none", "restart_service", "alert_admin", etc.
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
