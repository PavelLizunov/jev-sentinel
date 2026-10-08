//! Explicit guest power-state evidence from this collection's Proxmox resource probe.
use crate::config::ProxmoxGuestBinding;
use crate::core::models::{InfrastructureSnapshot, TargetStatus};
use chrono::Utc;
use serde_json::json;

pub fn apply_guest_power_state(
    snapshot: &mut InfrastructureSnapshot,
    bindings: &[ProxmoxGuestBinding],
) {
    let now = Utc::now();
    for binding in bindings {
        let Some(source) = snapshot
            .targets
            .iter()
            .find(|t| t.target_name == binding.source)
        else {
            continue;
        };
        let age = now
            .signed_duration_since(source.timestamp)
            .num_milliseconds();
        if source.status != TargetStatus::Online
            || source.error_message.is_some()
            || source.observed_at.is_none()
            || age < 0
            || age > binding.max_age_seconds as i64 * 1000
        {
            continue;
        }
        let Some(rows) = source.metrics.as_array() else {
            continue;
        };
        let matches: Vec<_> = rows
            .iter()
            .filter(|r| {
                r.get("vmid").and_then(|v| v.as_u64()) == Some(binding.vmid)
                    && r.get("type").and_then(|v| v.as_str()) == Some(binding.guest_type.as_str())
            })
            .collect();
        if matches.len() != 1
            || matches[0].get("status").and_then(|v| v.as_str()) != Some("stopped")
        {
            continue;
        }
        let metrics = json!({"vmid": binding.vmid, "type": binding.guest_type,
            "node": matches[0].get("node"), "status": "stopped",
            "power_state_source": binding.source});
        let timestamp = source.timestamp;
        let observed_at = source.observed_at;
        let Some(target) = snapshot
            .targets
            .iter_mut()
            .find(|t| t.target_name == binding.target)
        else {
            continue;
        };
        // A reachable service contradicts a stopped guest: do not hide that observation.
        if !matches!(
            target.status,
            TargetStatus::Unknown | TargetStatus::Timeout | TargetStatus::Unreachable
        ) {
            continue;
        }
        target.status = TargetStatus::PoweredOff;
        target.metrics = metrics;
        target.error_message = None;
        target.timestamp = timestamp;
        target.observed_at = observed_at;
        target.latency_ms = 0.0; // Publication suppresses latency for powered-off guests.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::TargetTelemetry;
    use chrono::Duration;
    use std::time::Instant;
    fn binding() -> ProxmoxGuestBinding {
        ProxmoxGuestBinding {
            target: "worker".into(),
            source: "cluster".into(),
            vmid: 101,
            guest_type: "qemu".into(),
            max_age_seconds: 90,
        }
    }
    fn snapshot() -> InfrastructureSnapshot {
        let telemetry = |name: &str, status, metrics| TargetTelemetry {
            target_name: name.into(),
            target_type: "exec_probe".into(),
            status,
            latency_ms: 1.0,
            metrics,
            error_message: None,
            timestamp: Utc::now(),
            observed_at: Some(Instant::now()),
        };
        InfrastructureSnapshot {
            timestamp: Utc::now(),
            targets: vec![
                telemetry(
                    "cluster",
                    TargetStatus::Online,
                    json!([{"vmid":101,"type":"qemu","status":"stopped","node":"node1"}]),
                ),
                telemetry("worker", TargetStatus::Timeout, json!({})),
            ],
        }
    }
    #[test]
    fn confirms_only_fresh_explicit_stopped_guest() {
        let mut s = snapshot();
        apply_guest_power_state(&mut s, &[binding()]);
        assert_eq!(s.targets[1].status, TargetStatus::PoweredOff);
        assert_eq!(s.targets[1].metrics["node"], "node1");
        assert_eq!(s.targets[1].timestamp, s.targets[0].timestamp);
        assert!(s.targets[1].error_message.is_none());
    }
    #[test]
    fn unavailable_stale_future_running_ambiguous_or_wrong_type_do_not_claim_off() {
        for case in 0..9 {
            let mut s = snapshot();
            match case {
                0 => s.targets[0].status = TargetStatus::Timeout,
                1 => s.targets[0].timestamp -= Duration::seconds(91),
                2 => s.targets[0].timestamp += Duration::seconds(5),
                3 => s.targets[0].metrics[0]["status"] = json!("running"),
                4 => s.targets[0].metrics[0]["type"] = json!("lxc"),
                5 => s.targets[0]
                    .metrics
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"vmid":101,"type":"qemu","status":"stopped"})),
                6 => s.targets[0].metrics[0]["vmid"] = json!(102),
                7 => s.targets[0].observed_at = None,
                _ => s.targets[0].error_message = Some("partial".into()),
            }
            apply_guest_power_state(&mut s, &[binding()]);
            assert_eq!(s.targets[1].status, TargetStatus::Timeout, "case {case}");
        }
    }
    #[test]
    fn reachable_target_and_unbound_target_are_not_overridden() {
        let mut s = snapshot();
        s.targets[1].status = TargetStatus::Online;
        apply_guest_power_state(&mut s, &[binding()]);
        assert_eq!(s.targets[1].status, TargetStatus::Online);
        let mut s = snapshot();
        apply_guest_power_state(&mut s, &[]);
        assert_eq!(s.targets[1].status, TargetStatus::Timeout);
        let mut s = snapshot();
        s.targets[0].metrics[0]["status"] = json!("running");
        apply_guest_power_state(&mut s, &[binding()]);
        assert_eq!(s.targets[1].status, TargetStatus::Timeout);
    }
}
