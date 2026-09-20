use crate::core::history::{
    HistoryPoint, NormalizedMetrics, Ring, Sample, HISTORY_CAPACITY, VISIBLE_SAMPLES,
};
use crate::core::models::{
    InfrastructureSnapshot, SentinelDecision, TargetStatus, TargetTelemetry,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CategoryDefinition {
    pub id: String,
    pub label: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_key: Option<String>,
}

pub fn default_categories() -> Vec<CategoryDefinition> {
    [
        (
            "ai_compute",
            "AI & Model Inference",
            "GPU servers, LLM inference endpoints, local models, voice synthesis",
            "category.ai_compute",
        ),
        (
            "virtualization",
            "Hypervisors & Storage",
            "Proxmox cluster nodes, virtual machines, cluster quorum, shared NFS",
            "category.virtualization",
        ),
        (
            "network_edge",
            "Ingress & Network Egress",
            "Caddy reverse proxy, egress gateways (gost/xray), VPN tunnels, DNS",
            "category.network_edge",
        ),
        (
            "observability",
            "Monitoring & Observability",
            "Monitoring dashboards (Beszel, Pulse, Hermes), metrics exporters",
            "category.observability",
        ),
        (
            "workstations",
            "Workstations & Desktops",
            "Developer desktops, jump hosts, test desktop VMs, client machines",
            "category.workstations",
        ),
    ]
    .into_iter()
    .map(|(id, label, description, label_key)| CategoryDefinition {
        id: id.into(),
        label: label.into(),
        description: description.into(),
        label_key: Some(label_key.into()),
    })
    .collect()
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObservationState {
    Healthy,
    Degraded,
    Failed,
    Unknown,
    Stale,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdvisorState {
    Ready,
    Evaluating,
    Unavailable,
    InvalidResponse,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublishedStatus {
    pub schema_version: u8,
    pub daemon_epoch: String,
    pub view_revision: String,
    pub cycle_id: u64,
    pub timestamp: DateTime<Utc>,
    pub phase: String,
    pub advisor_state: AdvisorState,
    pub system_health: String,
    pub health_confidence: Option<f64>,
    pub risk_score: Option<f64>,
    pub action_required: Option<bool>,
    pub suggested_action: Option<String>,
    pub jev_latency_ms: Option<f64>,
    pub collection_latency_ms: Option<f64>,
    pub stale_after_ms: u64,
    pub online_targets: usize,
    pub total_targets: usize,
    pub categories: Vec<CategoryDefinition>,
    pub targets: Vec<PublishedTarget>,
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}
impl Default for PublishedStatus {
    fn default() -> Self {
        Self {
            schema_version: 2,
            daemon_epoch: String::new(),
            view_revision: "0".into(),
            cycle_id: 0,
            timestamp: Utc::now(),
            phase: "starting".into(),
            advisor_state: AdvisorState::Unavailable,
            system_health: "unknown".into(),
            health_confidence: None,
            risk_score: None,
            action_required: None,
            suggested_action: None,
            jev_latency_ms: None,
            collection_latency_ms: None,
            stale_after_ms: 120_000,
            online_targets: 0,
            total_targets: 0,
            categories: default_categories(),
            targets: Vec::new(),
            error_message: None,
            error_code: None,
        }
    }
}
impl PublishedStatus {
    /// HTTP reachability does not refresh telemetry. Age only follows collector time.
    pub fn refreshed_at(&self, now: Instant) -> Self {
        let mut result = self.clone();
        for t in &mut result.targets {
            t.age_ms = t
                .observed_at
                .and_then(|at| now.checked_duration_since(at))
                .map(|d| d.as_millis().min(u64::MAX as u128) as u64);
            t.state = match t.age_ms {
                Some(age) if age > result.stale_after_ms => ObservationState::Stale,
                Some(_) => t.observed_state,
                None => ObservationState::Unknown,
            };
        }
        result.online_targets = result
            .targets
            .iter()
            .filter(|t| t.state == ObservationState::Healthy)
            .count();
        result
    }
    pub fn set_decision(&mut self, decision: &SentinelDecision, elapsed_ms: f64) {
        self.phase = "ready".into();
        self.advisor_state = AdvisorState::Ready;
        self.system_health = decision.system_health.clone();
        self.health_confidence = decision.health_confidence;
        self.risk_score = Some(decision.risk_score);
        self.action_required = Some(decision.action_required);
        self.suggested_action = Some(decision.suggested_action.clone());
        self.jev_latency_ms = Some(elapsed_ms);
        self.error_message = None;
        self.error_code = None;
    }
    pub fn set_error(&mut self, invalid_response: bool) {
        self.phase = "evaluation_error".into();
        self.advisor_state = if invalid_response {
            AdvisorState::InvalidResponse
        } else {
            AdvisorState::Unavailable
        };
        self.system_health = "unknown".into();
        self.health_confidence = None;
        self.risk_score = None;
        self.action_required = None;
        self.suggested_action = None;
        self.jev_latency_ms = None;
        self.error_message = Some(
            if invalid_response {
                "Jev returned an invalid decision; no advice is available."
            } else {
                "Jev is unavailable; probe observations remain independent."
            }
            .into(),
        );
        self.error_code = Some(
            if invalid_response {
                "invalid_decision"
            } else {
                "jev_unavailable"
            }
            .into(),
        );
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PublishedTarget {
    pub id: String,
    pub name: String,
    pub target_type: String,
    pub category: Option<String>,
    pub status: TargetStatus,
    pub state: ObservationState,
    pub measured_at: DateTime<Utc>,
    pub age_ms: Option<u64>,
    pub sample_seq: String,
    pub latency_ms: Option<f64>,
    pub normalized: NormalizedMetrics,
    pub history: Vec<HistoryPoint>,
    pub metrics: serde_json::Value,
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip)]
    pub observed_at: Option<Instant>,
    #[serde(skip)]
    pub observed_state: ObservationState,
}

/// The configured unique name is the probe identity; never a guessed machine identity.
pub fn probe_id(t: &TargetTelemetry) -> String {
    format!("{}:{}", t.target_type, t.target_name)
}

fn observation(t: &TargetTelemetry) -> ObservationState {
    if t.metrics.get("internal_error").is_some()
        || t.metrics.get("parse_error").and_then(|v| v.as_bool()) == Some(true)
    {
        return ObservationState::Unknown;
    }
    match t.status {
        TargetStatus::Online => ObservationState::Healthy,
        TargetStatus::Degraded => ObservationState::Degraded,
        TargetStatus::Unreachable
            if t.metrics
                .get("status_code")
                .and_then(|v| v.as_u64())
                .is_some() =>
        {
            ObservationState::Failed
        }
        TargetStatus::Timeout | TargetStatus::Unreachable | TargetStatus::Unknown => {
            ObservationState::Unknown
        }
    }
}

/// Public metrics are deliberately bounded and allowlisted. Raw commands, URLs,
/// stdout/stderr and arbitrary provider error strings are not dashboard data.
fn public_metrics(m: &serde_json::Value) -> serde_json::Value {
    const KEYS: &[&str] = &[
        "vmid",
        "type",
        "node",
        "name",
        "status",
        "mem",
        "maxmem",
        "cpu",
        "maxcpu",
        "status_code",
        "expected_status",
        "connected",
        "exit_code",
        "truncated",
        "gpu_name",
        "gpu_count",
        "temperature_c",
        "memory_used_mb",
        "memory_total_mb",
        "utilization_pct",
        "swap_used_mb",
        "physical_memory_mb",
        "free_ram_mb",
        "wired_ram_mb",
        "active_ram_mb",
        "parse_error",
    ];
    if let Some(rows) = m.as_array() {
        return rows
            .iter()
            .take(128)
            .map(public_metrics)
            .collect::<Vec<_>>()
            .into();
    }
    let mut out = serde_json::Map::new();
    if let Some(services) = m.get("services").and_then(|v| v.as_object()) {
        let values: serde_json::Map<String, serde_json::Value> = services
            .iter()
            .take(64)
            .filter(|(_, v)| v.is_boolean())
            .map(|(k, v)| (k.chars().take(128).collect(), v.clone()))
            .collect();
        out.insert("services".into(), values.into());
    }
    for key in KEYS {
        if let Some(v) = m.get(*key) {
            if v.is_number() || v.is_boolean() || v.is_null() {
                out.insert((*key).into(), v.clone());
            } else if let Some(s) = v.as_str() {
                out.insert(
                    (*key).into(),
                    s.chars().take(128).collect::<String>().into(),
                );
            }
        }
    }
    out.into()
}

#[derive(Default)]
struct Series {
    ring: Ring<Sample, HISTORY_CAPACITY>,
    last_observed: Option<Instant>,
    sequence: u64,
}
pub struct DashboardHistory {
    started: Instant,
    epoch: String,
    stale_after_ms: u64,
    series: HashMap<String, Series>,
}
impl DashboardHistory {
    pub fn new(interval_seconds: u64) -> Self {
        Self {
            started: Instant::now(),
            epoch: format!(
                "{}-{}",
                Utc::now().format("%Y%m%dT%H%M%S%.9f"),
                std::process::id()
            ),
            stale_after_ms: interval_seconds
                .saturating_mul(2)
                .max(30)
                .saturating_mul(1000),
            series: HashMap::new(),
        }
    }
    pub fn initial(&self) -> PublishedStatus {
        PublishedStatus {
            daemon_epoch: self.epoch.clone(),
            stale_after_ms: self.stale_after_ms,
            ..PublishedStatus::default()
        }
    }
    pub fn observe(
        &mut self,
        cycle_id: u64,
        snapshot: &InfrastructureSnapshot,
        collection_ms: f64,
    ) -> PublishedStatus {
        // Config validation bounds targets to 1024; retain no history for removed probes.
        self.series
            .retain(|id, _| snapshot.targets.iter().any(|t| probe_id(t) == *id));
        let now = Instant::now();
        let mut targets = Vec::with_capacity(snapshot.targets.len().min(1024));
        for t in snapshot.targets.iter().take(1024) {
            let id = probe_id(t);
            let metrics = NormalizedMetrics::from_telemetry(t);
            let series = self.series.entry(id.clone()).or_default();
            let at = t.observed_at;
            if let Some(at) = at.filter(|at| series.last_observed.is_none_or(|last| *at > last)) {
                let elapsed = at
                    .checked_duration_since(self.started)
                    .unwrap_or_default()
                    .as_millis() as u64;
                series
                    .ring
                    .push(Sample::from_telemetry(t, &metrics, elapsed));
                series.sequence = series.sequence.saturating_add(1);
                series.last_observed = Some(at);
            }
            let mut previous = None;
            let history = series
                .ring
                .recent(VISIBLE_SAMPLES)
                .map(|s| {
                    let gap =
                        previous.is_some_and(|p| s.t_ms.saturating_sub(p) > self.stale_after_ms);
                    previous = Some(s.t_ms);
                    s.point(gap)
                })
                .collect();
            let observed_state = observation(t);
            let latency_ms = if matches!(t.status, TargetStatus::Online | TargetStatus::Degraded)
                && t.latency_ms.is_finite()
                && t.latency_ms >= 0.0
            {
                Some(t.latency_ms)
            } else {
                None
            };
            let (error_message, error_code) = match t.error_message.as_ref() {
                Some(_) => match observed_state {
                    ObservationState::Failed => (
                        Some("Probe received an unexpected HTTP status.".into()),
                        Some("unexpected_http_status".into()),
                    ),
                    ObservationState::Degraded => (
                        Some("Probe reported degraded conditions. Check local logs for details.".into()),
                        Some("probe_degraded".into()),
                    ),
                    _ => (
                        Some("Observation unavailable. Check observer path and collector logs; service failure is not confirmed.".into()),
                        Some("observation_unavailable".into()),
                    ),
                },
                None => (None, None),
            };
            targets.push(PublishedTarget {
                id,
                name: t.target_name.clone(),
                target_type: t.target_type.clone(),
                category: None,
                status: t.status.clone(),
                state: observed_state,
                observed_state,
                observed_at: at,
                measured_at: t.timestamp,
                age_ms: None,
                sample_seq: series.sequence.to_string(),
                latency_ms,
                normalized: metrics,
                history,
                metrics: public_metrics(&t.metrics),
                error_message,
                error_code,
            });
        }
        targets.sort_by(|a, b| a.id.cmp(&b.id));
        PublishedStatus {
            cycle_id,
            timestamp: snapshot.timestamp,
            phase: "evaluating".into(),
            advisor_state: AdvisorState::Evaluating,
            collection_latency_ms: Some(collection_ms),
            total_targets: targets.len(),
            targets,
            ..self.initial()
        }
        .refreshed_at(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn snapshot(at: Instant, status: TargetStatus) -> InfrastructureSnapshot {
        InfrastructureSnapshot {
            timestamp: Utc::now(),
            targets: vec![TargetTelemetry {
                target_name: "one".into(),
                target_type: "tcp_ping".into(),
                status,
                latency_ms: 3.0,
                metrics: serde_json::json!({"raw_output":"secret", "url":"https://secret", "cpu":0.1}),
                error_message: None,
                timestamp: Utc::now(),
                observed_at: Some(at),
            }],
        }
    }
    #[test]
    fn http_polls_and_advisor_updates_cannot_refresh_observations() {
        let mut store = DashboardHistory::new(10);
        let at = Instant::now();
        let s = snapshot(at, TargetStatus::Online);
        let mut view = store.observe(1, &s, 1.0);
        assert_eq!(view.targets[0].history.len(), 1);
        assert_eq!(store.observe(1, &s, 1.0).targets[0].history.len(), 1);
        view.set_error(true);
        assert_eq!(view.risk_score, None);
        assert_eq!(view.targets[0].state, ObservationState::Healthy);
        let stale = view.refreshed_at(at + Duration::from_secs(31));
        assert_eq!(stale.targets[0].state, ObservationState::Stale);
        assert_eq!(stale.online_targets, 0);
        assert_eq!(stale.targets[0].metrics.get("raw_output"), None);
    }
    #[test]
    fn wall_clock_jump_does_not_change_monotonic_age() {
        let mut store = DashboardHistory::new(10);
        let at = Instant::now();
        let mut s = snapshot(at, TargetStatus::Online);
        s.targets[0].timestamp -= chrono::Duration::days(100);
        let view = store
            .observe(1, &s, 1.0)
            .refreshed_at(at + Duration::from_secs(5));
        assert_eq!(view.targets[0].age_ms, Some(5000));
        assert_eq!(view.targets[0].state, ObservationState::Healthy);
    }

    #[test]
    fn timeout_is_unknown_not_failed_and_restart_resets_history() {
        let mut a = DashboardHistory::new(60);
        let s = snapshot(Instant::now(), TargetStatus::Timeout);
        let v = a.observe(1, &s, 1.0);
        assert_eq!(v.targets[0].state, ObservationState::Unknown);
        assert_eq!(v.targets[0].latency_ms, None);
        let mut b = DashboardHistory::new(60);
        assert!(b.initial().targets.is_empty());
        assert_eq!(b.observe(1, &s, 1.0).targets[0].sample_seq, "1");
    }
    #[test]
    fn long_gaps_are_explicit_and_api_never_exposes_more_than_twenty_samples() {
        let mut store = DashboardHistory::new(10);
        let at = Instant::now();
        for i in 0..140 {
            let s = snapshot(at + Duration::from_secs(i * 10), TargetStatus::Online);
            let v = store.observe(i, &s, 1.0);
            assert!(v.targets[0].history.len() <= VISIBLE_SAMPLES);
        }
        let s = snapshot(at + Duration::from_secs(1500), TargetStatus::Online);
        let v = store.observe(141, &s, 1.0);
        assert!(v.targets[0].history.last().unwrap().break_before);
        assert_eq!(
            store
                .series
                .values()
                .next()
                .unwrap()
                .ring
                .recent(1000)
                .count(),
            128
        );
        assert_eq!(v.targets[0].sample_seq, "141");
    }

    #[test]
    fn histories_survive_reordering_but_not_removed_targets() {
        let mut store = DashboardHistory::new(10);
        let at = Instant::now();
        let mut s = snapshot(at, TargetStatus::Online);
        let mut second = s.targets[0].clone();
        second.target_name = "two".into();
        s.targets.push(second);
        let ids: Vec<_> = store
            .observe(1, &s, 1.0)
            .targets
            .iter()
            .map(|t| t.id.clone())
            .collect();
        s.targets.reverse();
        assert_eq!(
            store
                .observe(2, &s, 1.0)
                .targets
                .iter()
                .map(|t| t.id.clone())
                .collect::<Vec<_>>(),
            ids
        );
        s.targets.clear();
        store.observe(3, &s, 1.0);
        assert!(store.series.is_empty());
    }

    #[test]
    fn test_localization_metadata_status_and_target_error_codes() {
        let mut status = PublishedStatus::default();
        assert_eq!(status.error_message, None);
        assert_eq!(status.error_code, None);
        assert_eq!(status.schema_version, 2);

        // invalid_response error
        status.set_error(true);
        assert_eq!(status.error_code, Some("invalid_decision".into()));
        assert_eq!(
            status.error_message,
            Some("Jev returned an invalid decision; no advice is available.".into())
        );

        // Reset with decision
        let decision = SentinelDecision {
            timestamp: Utc::now(),
            system_health: "healthy".into(),
            health_confidence: Some(0.95),
            risk_score: 0.1,
            action_required: false,
            action_probability: 0.05,
            suggested_action: "none".into(),
            raw_answers: HashMap::new(),
        };
        status.set_decision(&decision, 50.0);
        assert_eq!(status.error_code, None);
        assert_eq!(status.error_message, None);

        // jev_unavailable error
        status.set_error(false);
        assert_eq!(status.error_code, Some("jev_unavailable".into()));
        assert_eq!(
            status.error_message,
            Some("Jev is unavailable; probe observations remain independent.".into())
        );

        // Target error codes
        let mut store = DashboardHistory::new(10);
        let at = Instant::now();

        // 1. Degraded
        let mut s_deg = snapshot(at, TargetStatus::Degraded);
        s_deg.targets[0].error_message = Some("Degraded error".into());
        let view_deg = store.observe(1, &s_deg, 1.0);
        assert_eq!(
            view_deg.targets[0].error_code,
            Some("probe_degraded".into())
        );
        assert_eq!(
            view_deg.targets[0].error_message,
            Some("Probe reported degraded conditions. Check local logs for details.".into())
        );

        // 2. Failed (status_code present in metrics + unreachable)
        let mut s_failed = snapshot(at, TargetStatus::Unreachable);
        s_failed.targets[0].metrics = serde_json::json!({"status_code": 500});
        s_failed.targets[0].error_message = Some("HTTP 500".into());
        let view_failed = store.observe(2, &s_failed, 1.0);
        assert_eq!(
            view_failed.targets[0].error_code,
            Some("unexpected_http_status".into())
        );
        assert_eq!(
            view_failed.targets[0].error_message,
            Some("Probe received an unexpected HTTP status.".into())
        );

        // 3. Unknown / observation_unavailable
        let mut s_unknown = snapshot(at, TargetStatus::Timeout);
        s_unknown.targets[0].error_message = Some("Timeout".into());
        let view_unknown = store.observe(3, &s_unknown, 1.0);
        assert_eq!(
            view_unknown.targets[0].error_code,
            Some("observation_unavailable".into())
        );
        assert_eq!(
            view_unknown.targets[0].error_message,
            Some(
                "Observation unavailable. Check observer path and collector logs; service failure is not confirmed."
                    .into()
            )
        );

        // 4. Healthy target without error_message resets to None
        let s_ok = snapshot(at, TargetStatus::Online);
        let view_ok = store.observe(4, &s_ok, 1.0);
        assert_eq!(view_ok.targets[0].error_code, None);
        assert_eq!(view_ok.targets[0].error_message, None);
    }

    #[test]
    fn test_category_localization_and_serialization() {
        let cats = default_categories();
        assert_eq!(cats.len(), 5);
        for c in &cats {
            assert!(c.label_key.is_some());
            assert_eq!(c.label_key, Some(format!("category.{}", c.id)));
        }

        // Custom category deserialized without label_key
        let json_no_key = r#"{"id":"custom","label":"My Label","description":"My Desc"}"#;
        let parsed: CategoryDefinition = serde_json::from_str(json_no_key).unwrap();
        assert_eq!(parsed.label_key, None);

        // Custom category with forged label_key
        let json_forged = r#"{"id":"custom","label":"My Label","description":"My Desc","label_key":"category.virtualization"}"#;
        let mut parsed_forged: CategoryDefinition = serde_json::from_str(json_forged).unwrap();
        assert_eq!(
            parsed_forged.label_key,
            Some("category.virtualization".into())
        );
        // Stripping untrusted label_key
        parsed_forged.label_key = None;
        assert_eq!(parsed_forged.label_key, None);

        // Serialization skips None label_key
        let serialized = serde_json::to_string(&parsed_forged).unwrap();
        assert!(!serialized.contains("label_key"));

        // Serialization includes Some label_key for builtins
        let serialized_builtin = serde_json::to_string(&cats[0]).unwrap();
        assert!(serialized_builtin.contains("\"label_key\":\"category.ai_compute\""));
    }
}
