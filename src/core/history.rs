//! Bounded numeric history. No raw JSON or strings are retained per sample.
use crate::core::models::{TargetStatus, TargetTelemetry};
use serde::Serialize;
use serde_json::Value;

pub const HISTORY_CAPACITY: usize = 128;
pub const VISIBLE_SAMPLES: usize = 20;
const LATENCY: u16 = 1;
const AVAILABLE: u16 = 2;
const SWAP: u16 = 4;
const CPU: u16 = 8;
const RAM: u16 = 16;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Sample {
    pub t_ms: u64,
    pub latency_us: u32,
    pub available_mib: u32,
    pub swap_used_mib: u32,
    pub swapout_kib_s: u32,
    pub io_await_us: u32,
    pub io_queue_milli: u32,
    pub cpu_bp: u16,
    pub ram_used_bp: u16,
    pub valid: u16,
    pub pressure: u8,
    pub status: u8,
}

pub struct Ring<T: Copy + Default, const N: usize> {
    slots: [T; N],
    next: usize,
    len: usize,
}
impl<T: Copy + Default, const N: usize> Default for Ring<T, N> {
    fn default() -> Self {
        assert!(N > 0);
        Self {
            slots: [T::default(); N],
            next: 0,
            len: 0,
        }
    }
}
impl<T: Copy + Default, const N: usize> Ring<T, N> {
    pub fn push(&mut self, value: T) {
        self.slots[self.next] = value;
        self.next = (self.next + 1) % N;
        self.len = (self.len + 1).min(N);
    }
    pub fn recent(&self, count: usize) -> impl Iterator<Item = &T> {
        let count = count.min(self.len);
        let start = (self.next + N - count) % N;
        (0..count).map(move |i| &self.slots[(start + i) % N])
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct NormalizedMetrics {
    pub cpu_pct: Option<f64>,
    pub ram_used_pct: Option<f64>,
    pub available_mib: Option<f64>,
    pub swap_used_mib: Option<f64>,
    pub total_memory_mib: Option<f64>,
    pub gpu_used_pct: Option<f64>,
    pub gpu_util_pct: Option<f64>,
    pub memory_basis: Option<&'static str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_basis_code: Option<&'static str>,
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value
        .get(key)?
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
}
fn percent(value: Option<f64>) -> Option<f64> {
    value.filter(|n| *n <= 100.0)
}
fn ratio(used: Option<f64>, total: Option<f64>) -> Option<f64> {
    let (used, total) = (used?, total?);
    if total > 0.0 && used <= total {
        Some(100.0 * (used / total))
    } else {
        None
    }
}

impl NormalizedMetrics {
    pub fn from_telemetry(t: &TargetTelemetry) -> Self {
        if !matches!(t.status, TargetStatus::Online | TargetStatus::Degraded) {
            return Self::default();
        }
        let m = &t.metrics;
        // Aggregate Proxmox arrays are deliberately not guessed to belong to this probe.
        // They remain available in Details with their explicit resource IDs.
        if !m.is_object() {
            return Self::default();
        }
        if t.target_type == "macos_ssh" {
            let total = number(m, "physical_memory_mb").filter(|n| *n > 0.0);
            let available = number(m, "free_ram_mb");
            return Self {
                available_mib: available,
                swap_used_mib: number(m, "swap_used_mb"),
                total_memory_mib: total,
                ram_used_pct: ratio(
                    total
                        .zip(available)
                        .and_then(|(t, a)| (a <= t).then_some(t - a)),
                    total,
                ),
                memory_basis: Some("Darwin free + inactive estimate"),
                memory_basis_code: Some("darwin_available_estimate"),
                ..Self::default()
            };
        }
        if t.target_type == "nvidia_ssh" {
            return Self {
                gpu_used_pct: ratio(number(m, "memory_used_mb"), number(m, "memory_total_mb")),
                gpu_util_pct: percent(number(m, "utilization_pct")),
                ..Self::default()
            };
        }
        let total =
            number(m, "memory_total_mib").or_else(|| number(m, "maxmem").map(|n| n / 1048576.0));
        let available = number(m, "memory_available_mib");
        let used = number(m, "memory_used_mib").or_else(|| number(m, "mem").map(|n| n / 1048576.0));
        Self {
            cpu_pct: percent(number(m, "cpu_pct").or_else(|| number(m, "cpu").map(|n| n * 100.0))),
            ram_used_pct: percent(number(m, "ram_used_pct")).or_else(|| ratio(used, total)),
            available_mib: available,
            swap_used_mib: number(m, "swap_used_mib"),
            total_memory_mib: total.filter(|n| *n > 0.0),
            memory_basis: Some("reported by probe"),
            memory_basis_code: Some("reported_by_probe"),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct HistoryPoint {
    pub t_ms: u64,
    pub latency_ms: Option<f64>,
    pub ram_pct: Option<f64>,
    pub available_mib: Option<u32>,
    pub swap_mib: Option<u32>,
    pub break_before: bool,
}
impl Sample {
    pub fn from_telemetry(t: &TargetTelemetry, m: &NormalizedMetrics, t_ms: u64) -> Self {
        let mut s = Self {
            t_ms,
            ..Self::default()
        };
        let successful = matches!(t.status, TargetStatus::Online | TargetStatus::Degraded);
        if successful
            && t.latency_ms.is_finite()
            && t.latency_ms >= 0.0
            && t.latency_ms * 1000.0 <= u32::MAX as f64
        {
            s.latency_us = (t.latency_ms * 1000.0).round() as u32;
            s.valid |= LATENCY;
        }
        for (value, field, bit) in [
            (m.available_mib, &mut s.available_mib, AVAILABLE),
            (m.swap_used_mib, &mut s.swap_used_mib, SWAP),
        ] {
            if let Some(v) = value.filter(|v| *v <= u32::MAX as f64) {
                *field = v.round() as u32;
                s.valid |= bit;
            }
        }
        if let Some(v) = m.cpu_pct {
            s.cpu_bp = (v * 100.0).round() as u16;
            s.valid |= CPU;
        }
        if let Some(v) = m.ram_used_pct {
            s.ram_used_bp = (v * 100.0).round() as u16;
            s.valid |= RAM;
        }
        s.status = if successful { 1 } else { 0 };
        s
    }
    pub fn point(&self, break_before: bool) -> HistoryPoint {
        HistoryPoint {
            t_ms: self.t_ms,
            latency_ms: (self.valid & LATENCY != 0).then_some(self.latency_us as f64 / 1000.0),
            ram_pct: (self.valid & RAM != 0).then_some(self.ram_used_bp as f64 / 100.0),
            available_mib: (self.valid & AVAILABLE != 0).then_some(self.available_mib),
            swap_mib: (self.valid & SWAP != 0).then_some(self.swap_used_mib),
            break_before,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn telemetry(metrics: Value) -> TargetTelemetry {
        TargetTelemetry {
            target_name: "test".into(),
            target_type: "exec_probe".into(),
            status: TargetStatus::Online,
            latency_ms: 2.0,
            metrics,
            error_message: None,
            timestamp: chrono::Utc::now(),
            observed_at: Some(std::time::Instant::now()),
        }
    }
    #[test]
    fn ring_is_bounded_and_ordered_after_wrap() {
        assert_eq!(std::mem::size_of::<Sample>(), 40);
        let mut r = Ring::<u32, 3>::default();
        assert_eq!(r.recent(20).count(), 0);
        for i in 0..10 {
            r.push(i);
        }
        assert_eq!(r.recent(20).copied().collect::<Vec<_>>(), vec![7, 8, 9]);
        assert_eq!(r.recent(2).copied().collect::<Vec<_>>(), vec![8, 9]);
    }
    #[test]
    fn zero_missing_and_timeout_are_distinct() {
        let mut t = telemetry(serde_json::json!({"swap_used_mib":0, "cpu_pct":0}));
        let m = NormalizedMetrics::from_telemetry(&t);
        assert_eq!(m.swap_used_mib, Some(0.0));
        assert_eq!(m.ram_used_pct, None);
        let s = Sample::from_telemetry(&t, &m, 100);
        assert_eq!(s.point(false).swap_mib, Some(0));
        t.status = TargetStatus::Timeout;
        let m = NormalizedMetrics::from_telemetry(&t);
        assert_eq!(
            Sample::from_telemetry(&t, &m, 200).point(false).latency_ms,
            None
        );
        assert_eq!(m.cpu_pct, None);
    }
    #[test]
    fn ratios_do_not_assume_capacity_or_match_names() {
        let mut t = telemetry(serde_json::json!({"mem":50, "maxmem":100, "cpu":0.25}));
        assert_eq!(
            NormalizedMetrics::from_telemetry(&t).ram_used_pct,
            Some(50.0)
        );
        t.metrics = serde_json::json!([{"name":"test","mem":50,"maxmem":100}]);
        assert_eq!(NormalizedMetrics::from_telemetry(&t).ram_used_pct, None);
        t.target_type = "macos_ssh".into();
        t.metrics = serde_json::json!({"free_ram_mb":4000,"swap_used_mb":3000});
        assert_eq!(NormalizedMetrics::from_telemetry(&t).ram_used_pct, None);
        t.metrics["physical_memory_mb"] = serde_json::json!(32000);
        assert_eq!(
            NormalizedMetrics::from_telemetry(&t).ram_used_pct,
            Some(87.5)
        );
    }
    #[test]
    fn test_memory_basis_code() {
        let mut t = telemetry(serde_json::json!({"mem":50, "maxmem":100, "cpu":0.25}));
        let m = NormalizedMetrics::from_telemetry(&t);
        assert_eq!(m.memory_basis, Some("reported by probe"));
        assert_eq!(m.memory_basis_code, Some("reported_by_probe"));

        t.target_type = "macos_ssh".into();
        t.metrics = serde_json::json!({"free_ram_mb":4000, "swap_used_mb":3000, "physical_memory_mb":32000});
        let m_mac = NormalizedMetrics::from_telemetry(&t);
        assert_eq!(m_mac.memory_basis, Some("Darwin free + inactive estimate"));
        assert_eq!(m_mac.memory_basis_code, Some("darwin_available_estimate"));

        t.target_type = "nvidia_ssh".into();
        let m_nv = NormalizedMetrics::from_telemetry(&t);
        assert_eq!(m_nv.memory_basis, None);
        assert_eq!(m_nv.memory_basis_code, None);
    }
}
