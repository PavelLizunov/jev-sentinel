use crate::config::TelegramAlertSettings;
use crate::core::models::{InfrastructureSnapshot, SentinelDecision, TargetStatus};
use crate::error::{Result, SentinelError};
use reqwest::{Client, Proxy};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tracing::{debug, error, info, warn};

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

pub fn sanitize_inline_field(input: &str, max_chars: usize) -> String {
    let single_line = input.replace(['\r', '\n'], " ");
    truncate_field(&single_line, max_chars)
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
    pub generation: u64,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IncidentId {
    pub key: DeduplicationKey,
    pub generation: u64,
}

fn compute_plan_hash(event: &AlertEvent) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    event.severity.hash(&mut hasher);
    event.reason_code.hash(&mut hasher);
    event.title.hash(&mut hasher);
    event.message.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone)]
pub struct GenerationRecord {
    pub plan_hash: u64,
    pub plan_severity: AlertSeverity,
    pub chunks: Vec<String>,
    pub confirmed_chunks: usize,
    pub attempts: usize,
    pub exhausted: bool,
    pub exhausted_at: Option<std::time::Instant>,
    pub delivered_cooldown: Option<(AlertSeverity, std::time::Instant)>,
}

impl Default for GenerationRecord {
    fn default() -> Self {
        Self {
            plan_hash: 0,
            plan_severity: AlertSeverity::Info,
            chunks: Vec::new(),
            confirmed_chunks: 0,
            attempts: 0,
            exhausted: false,
            exhausted_at: None,
            delivered_cooldown: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct IncidentState {
    pub current_generation: u64,
    pub records: HashMap<u64, GenerationRecord>,
}

impl IncidentState {
    pub fn prune_old_records(&mut self, protected_gens: &std::collections::HashSet<u64>) {
        let current_gen = self.current_generation;
        self.records.retain(|&gen, r| {
            if gen == current_gen || protected_gens.contains(&gen) {
                return true;
            }
            if let Some((_, sent_at)) = r.delivered_cooldown {
                if sent_at.elapsed() < Duration::from_secs(600) {
                    return true;
                }
            }
            if let Some(ex_at) = r.exhausted_at {
                if ex_at.elapsed() < Duration::from_secs(600) {
                    return true;
                }
            }
            false
        });

        // Safety cap: retain at most 32 non-protected historical generations per incident.
        // Protected generations (current, pending, in-flight) are NEVER evicted by this cap!
        let non_protected_count = self
            .records
            .keys()
            .filter(|&g| *g != current_gen && !protected_gens.contains(g))
            .count();
        if non_protected_count > 32 {
            let mut candidates: Vec<u64> = self
                .records
                .keys()
                .copied()
                .filter(|&g| g != current_gen && !protected_gens.contains(&g))
                .collect();
            candidates.sort_unstable(); // ascending (oldest first)
            let excess = non_protected_count - 32;
            for g in candidates.into_iter().take(excess) {
                self.records.remove(&g);
            }
        }
    }
}

#[derive(Default)]
pub struct DeliveryState {
    pub is_closed: bool,
    pub in_flight: std::collections::HashSet<IncidentId>,
    pub incidents: HashMap<DeduplicationKey, IncidentState>,
    pub pending: HashMap<IncidentId, AlertEvent>,
}

impl DeliveryState {
    pub fn prune_incident_records(&mut self) {
        let mut protected_by_key: HashMap<DeduplicationKey, std::collections::HashSet<u64>> =
            HashMap::new();
        // Protect all generations in pending
        for inc_id in self.pending.keys() {
            protected_by_key
                .entry(inc_id.key.clone())
                .or_default()
                .insert(inc_id.generation);
        }
        // Protect all generations in-flight
        for inc_id in &self.in_flight {
            protected_by_key
                .entry(inc_id.key.clone())
                .or_default()
                .insert(inc_id.generation);
        }
        for (key, inc) in self.incidents.iter_mut() {
            let empty_set = std::collections::HashSet::new();
            let protected_gens = protected_by_key.get(key).unwrap_or(&empty_set);
            inc.prune_old_records(protected_gens);
        }
    }
}

enum TelegramChunkOutcome {
    Success,
    RateLimit(String),
    FallbackRateLimit(String),
}

async fn cancellable<F, T>(
    fut: F,
    shutdown_rx: Option<&tokio::sync::watch::Receiver<bool>>,
) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    if let Some(rx) = shutdown_rx {
        let mut rx_clone = rx.clone();
        if *rx_clone.borrow_and_update() {
            return Err(SentinelError::Action("Interrupted by shutdown".into()));
        }
        tokio::select! {
            res = fut => res,
            wait_res = rx_clone.wait_for(|&s| s) => {
                match wait_res {
                    Ok(_) => Err(SentinelError::Action("Interrupted by shutdown".into())),
                    Err(_) => Err(SentinelError::Action("Shutdown channel closed".into())),
                }
            }
        }
    } else {
        fut.await
    }
}

pub struct Notifier {
    client: Client,
    telegram: Option<TelegramAlertSettings>,
    api_base: String,
    delivery_state: Mutex<DeliveryState>,
    rate_limit_until: Mutex<Option<std::time::Instant>>,
    notify: tokio::sync::Notify,
}

impl Notifier {
    pub fn new(telegram: Option<TelegramAlertSettings>) -> Self {
        Self::new_with_fallback(telegram, None)
    }

    pub fn new_with_fallback(
        telegram: Option<TelegramAlertSettings>,
        fallback_proxy: Option<&str>,
    ) -> Self {
        let api_base = std::env::var("TELEGRAM_API_BASE")
            .unwrap_or_else(|_| "https://api.telegram.org".to_string());
        Self::new_full(telegram, fallback_proxy, api_base)
    }

    pub fn new_with_api_base(
        telegram: Option<TelegramAlertSettings>,
        api_base: impl Into<String>,
    ) -> Self {
        Self::new_full(telegram, None, api_base.into())
    }

    pub fn new_full(
        telegram: Option<TelegramAlertSettings>,
        fallback_proxy: Option<&str>,
        api_base: String,
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

            let is_local = api_base.starts_with("http://127.0.0.1")
                || api_base.starts_with("http://localhost");

            if !is_local {
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
            }

            builder.build().unwrap_or_else(|_| Client::new())
        } else {
            Client::new()
        };

        Self {
            client,
            telegram,
            api_base,
            delivery_state: Mutex::new(DeliveryState::default()),
            rate_limit_until: Mutex::new(None),
            notify: tokio::sync::Notify::new(),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.telegram.is_some()
    }

    pub fn build_alert_event(
        &self,
        source: AlertSource,
        target_id: Option<String>,
        severity: AlertSeverity,
        reason_code: AlertReason,
        title: impl Into<String>,
        message: impl Into<String>,
    ) -> AlertEvent {
        let title = title.into();
        let message = message.into();
        let key = DeduplicationKey {
            source,
            target_id: target_id.clone(),
            reason_code,
        };
        let generation = {
            let mut state = self.delivery_state.lock().unwrap();
            let inc = state.incidents.entry(key).or_default();
            inc.current_generation
        };

        AlertEvent {
            source,
            target_id,
            severity,
            reason_code,
            generation,
            title,
            message,
            timestamp: chrono::Utc::now(),
        }
    }

    pub fn enqueue_alert(&self, event: AlertEvent) -> bool {
        let Some(tg) = &self.telegram else {
            return false;
        };

        let min_sev = tg.min_severity.to_lowercase();
        let should_alert = match min_sev.as_str() {
            "info" => true,
            "warning" => event.severity >= AlertSeverity::Warning,
            "critical" => event.severity >= AlertSeverity::Critical,
            _ => event.severity >= AlertSeverity::Warning,
        };

        if !should_alert {
            return false;
        }

        let key = DeduplicationKey {
            source: event.source,
            target_id: event.target_id.clone(),
            reason_code: event.reason_code,
        };

        let mut state = self.delivery_state.lock().unwrap();
        if state.is_closed {
            debug!("Rejected alert enqueue: notifier is closed");
            return false;
        }

        let incident = state.incidents.entry(key.clone()).or_default();

        // Discard obsolete event if target has already advanced generation beyond this event
        if event.generation < incident.current_generation {
            debug!(
                "Discarding obsolete event for {:?} (event gen {}, current gen {})",
                key, event.generation, incident.current_generation
            );
            return false;
        }

        // Suppress duplicate if this exact generation was delivered recently with same or higher severity
        if let Some(rec) = incident.records.get(&event.generation) {
            if let Some(&(prev_sev, sent_at)) = rec.delivered_cooldown.as_ref() {
                if event.severity <= prev_sev && sent_at.elapsed() < Duration::from_secs(300) {
                    debug!("Suppressed duplicate alert for {:?}", key);
                    return false;
                }
            }
            if rec.exhausted && event.severity <= rec.plan_severity {
                debug!(
                    "Discarding repeat for exhausted incident generation {:?} at severity {:?}",
                    key, rec.plan_severity
                );
                return false;
            }
        }

        let inc_id = IncidentId {
            key: key.clone(),
            generation: event.generation,
        };

        // Coalesce into bounded pending table within the same incident generation (max 64 entries)
        if let Some(existing) = state.pending.get_mut(&inc_id) {
            if event.severity > existing.severity {
                *existing = event; // Escalate severity and message!
            } else if event.severity == existing.severity {
                existing.timestamp = event.timestamp;
                existing.message = event.message;
            } else {
                // Lower severity arrives: keep peak severity, title and critical evidence!
                existing.timestamp = event.timestamp;
            }
        } else {
            if state.pending.len() >= 64 {
                // Priority admission: evict lower severity if available
                if let Some(evict_id) = state
                    .pending
                    .iter()
                    .filter(|(_, e)| e.severity < event.severity)
                    .map(|(k, _)| k.clone())
                    .next()
                {
                    state.pending.remove(&evict_id);
                    state.pending.insert(inc_id, event);
                } else {
                    warn!(
                        "Pending alert table full (64), dropping event for {:?}",
                        key
                    );
                    return false;
                }
            } else {
                state.pending.insert(inc_id, event);
            }
        }

        self.notify.notify_one();
        true
    }

    pub fn close_admission(&self) {
        if let Ok(mut state) = self.delivery_state.lock() {
            state.is_closed = true;
        }
        self.notify.notify_waiters();
    }

    pub fn pop_next_pending(&self) -> Option<AlertEvent> {
        let mut state = self.delivery_state.lock().unwrap();
        state.prune_incident_records();

        while !state.pending.is_empty() {
            let id_to_pop = state
                .pending
                .iter()
                .max_by_key(|(_, e)| e.severity)
                .map(|(k, _)| k.clone())?;

            let event = state.pending.remove(&id_to_pop)?;

            // Check if this incident generation was already delivered or exhausted at equal/higher severity
            if let Some(inc) = state.incidents.get(&id_to_pop.key) {
                if let Some(rec) = inc.records.get(&event.generation) {
                    if rec.exhausted && event.severity <= rec.plan_severity {
                        debug!(
                            "Discarding popped repeat for exhausted incident: {:?}",
                            id_to_pop
                        );
                        continue;
                    }
                    if let Some(&(delivered_sev, sent_at)) = rec.delivered_cooldown.as_ref() {
                        if event.severity <= delivered_sev
                            && sent_at.elapsed() < Duration::from_secs(300)
                        {
                            debug!(
                                "Discarding redundant pending repeat covered by delivered generation: {:?}",
                                id_to_pop
                            );
                            continue;
                        }
                    }
                }
            }

            state.in_flight.insert(id_to_pop);
            return Some(event);
        }
        None
    }

    pub fn release_in_flight(&self, event: &AlertEvent) {
        let inc_id = IncidentId {
            key: DeduplicationKey {
                source: event.source,
                target_id: event.target_id.clone(),
                reason_code: event.reason_code,
            },
            generation: event.generation,
        };
        if let Ok(mut state) = self.delivery_state.lock() {
            state.in_flight.remove(&inc_id);
            state.prune_incident_records();
        }
    }

    pub fn commit_delivery(&self, event: &AlertEvent) -> bool {
        let key = DeduplicationKey {
            source: event.source,
            target_id: event.target_id.clone(),
            reason_code: event.reason_code,
        };
        let inc_id = IncidentId {
            key: key.clone(),
            generation: event.generation,
        };

        let mut state = self.delivery_state.lock().unwrap();
        state.in_flight.remove(&inc_id);

        let is_current = if let Some(inc) = state.incidents.get_mut(&key) {
            let rec = inc.records.entry(event.generation).or_default();
            rec.delivered_cooldown = Some((event.severity, std::time::Instant::now()));
            inc.current_generation == event.generation
        } else {
            false
        };

        state.prune_incident_records();

        if is_current {
            debug!(
                "Committed delivery for current generation {:?} gen {}",
                key, event.generation
            );
            true
        } else {
            debug!(
                "Committed delivery for historical generation {:?} gen {}",
                key, event.generation
            );
            false
        }
    }

    pub fn recover_target(&self, target_id: &str) {
        let mut state = self.delivery_state.lock().unwrap();
        for (k, inc) in state.incidents.iter_mut() {
            if k.target_id.as_deref() == Some(target_id) {
                inc.current_generation = inc.current_generation.wrapping_add(1);
            }
        }
        state.prune_incident_records();
    }

    pub fn recover_advisor_unavailable(&self) {
        let mut state = self.delivery_state.lock().unwrap();
        let adv_key = DeduplicationKey {
            source: AlertSource::LocalRule,
            target_id: None,
            reason_code: AlertReason::AdvisorUnavailable,
        };
        if let Some(inc) = state.incidents.get_mut(&adv_key) {
            inc.current_generation = inc.current_generation.wrapping_add(1);
        }
        state.prune_incident_records();
    }

    pub fn recover_risk_elevated(&self) {
        let mut state = self.delivery_state.lock().unwrap();
        let risk_key = DeduplicationKey {
            source: AlertSource::JevAdvisor,
            target_id: None,
            reason_code: AlertReason::RiskElevated,
        };
        if let Some(inc) = state.incidents.get_mut(&risk_key) {
            inc.current_generation = inc.current_generation.wrapping_add(1);
        }
        state.prune_incident_records();
    }

    pub async fn run_worker(
        self: std::sync::Arc<Self>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            if *shutdown_rx.borrow() {
                break;
            }
            tokio::select! {
                _ = self.notify.notified() => {
                    while let Some(event) = self.pop_next_pending() {
                        let formatted = self.format_alert_event(&event);
                        if let Err(e) = self.send_alert_message(&event, &formatted, Some(&shutdown_rx)).await {
                            error!("Alert delivery failed after retries for {:?}: {}", event.target_id, e);
                            self.release_in_flight(&event);
                        } else {
                            self.commit_delivery(&event);
                        }
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }
        if let Ok(mut state) = self.delivery_state.lock() {
            state.is_closed = true;
        }
    }

    pub fn format_alert_event(&self, event: &AlertEvent) -> String {
        let icon = match event.severity {
            AlertSeverity::Info => "ℹ️",
            AlertSeverity::Warning => "⚠️",
            AlertSeverity::Critical => "🚨",
        };
        let safe_title = escape_html(&sanitize_inline_field(&event.title, 80));
        let mut lines = vec![format!("<b>{} Jev Sentinel: {}</b>", icon, safe_title)];
        if let Some(ref target) = event.target_id {
            lines.push(format!(
                "<b>Target:</b> <code>{}</code>",
                escape_html(&sanitize_inline_field(target, 80))
            ));
        }
        lines.push(format!("<b>Severity:</b> {:?}", event.severity));
        lines.push(format!("<b>Reason:</b> {:?}", event.reason_code));
        lines.push(format!(
            "<b>Timestamp:</b> {}",
            event.timestamp.to_rfc3339()
        ));
        lines.push("".to_string());
        lines.push(escape_html(&truncate_field(&event.message, 8000)));
        lines.join("\n")
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

        let inc_id = IncidentId {
            key: DeduplicationKey {
                source: event.source,
                target_id: event.target_id.clone(),
                reason_code: event.reason_code,
            },
            generation: event.generation,
        };
        {
            let mut state = self.delivery_state.lock().unwrap();
            state.in_flight.insert(inc_id);
        }

        let formatted = self.format_alert_event(event);
        let res = self.send_alert_message(event, &formatted, None).await;
        if res.is_err() {
            self.release_in_flight(event);
        }
        res?;
        Ok(self.commit_delivery(event))
    }

    fn split_into_chunks(&self, message_text: &str) -> Vec<String> {
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
        chunks
    }

    fn extract_retry_after(&self, text: &str) -> Option<u64> {
        if let Ok(err_json) = serde_json::from_str::<serde_json::Value>(text) {
            err_json
                .get("parameters")
                .and_then(|p| p.get("retry_after"))
                .and_then(|v| v.as_u64())
        } else {
            None
        }
    }

    pub fn record_rate_limit(&self, wait_sec: u64) {
        let new_deadline = std::time::Instant::now() + Duration::from_secs(wait_sec);
        let mut guard = self.rate_limit_until.lock().unwrap();
        *guard = Some(match *guard {
            Some(old) => old.max(new_deadline),
            None => new_deadline,
        });
    }

    pub fn rate_limit_remaining_duration(&self) -> Option<Duration> {
        let guard = self.rate_limit_until.lock().unwrap();
        guard.and_then(|until| {
            let now = std::time::Instant::now();
            if until > now {
                Some(until - now)
            } else {
                None
            }
        })
    }

    async fn wait_for_rate_limit(
        &self,
        shutdown_rx: Option<&tokio::sync::watch::Receiver<bool>>,
    ) -> Result<()> {
        loop {
            let wait = {
                let guard = self.rate_limit_until.lock().unwrap();
                if let Some(until) = *guard {
                    let now = std::time::Instant::now();
                    if until > now {
                        Some(until - now)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(duration) = wait {
                debug!(
                    "Observing Telegram rate limit embargo, sleeping {:?}",
                    duration
                );
                let sleep_fut = async {
                    tokio::time::sleep(duration).await;
                    Ok(())
                };
                cancellable(sleep_fut, shutdown_rx).await?;
            } else {
                break;
            }
        }
        Ok(())
    }

    pub async fn send_alert_message(
        &self,
        event: &AlertEvent,
        message_text: &str,
        shutdown_rx: Option<&tokio::sync::watch::Receiver<bool>>,
    ) -> Result<()> {
        let Some(tg) = &self.telegram else {
            return Ok(());
        };

        let inc_id = IncidentId {
            key: DeduplicationKey {
                source: event.source,
                target_id: event.target_id.clone(),
                reason_code: event.reason_code,
            },
            generation: event.generation,
        };

        let event_plan_hash = compute_plan_hash(event);
        let fresh_chunks = self.split_into_chunks(message_text);

        let (plan_chunks, starting_chunk, starting_attempt, is_exhausted) = {
            let mut state = self.delivery_state.lock().unwrap();
            if let Some(inc) = state.incidents.get_mut(&inc_id.key) {
                let rec =
                    inc.records
                        .entry(inc_id.generation)
                        .or_insert_with(|| GenerationRecord {
                            plan_hash: event_plan_hash,
                            plan_severity: event.severity,
                            chunks: fresh_chunks.clone(),
                            confirmed_chunks: 0,
                            attempts: 0,
                            exhausted: false,
                            exhausted_at: None,
                            delivered_cooldown: None,
                        });

                if event.severity > rec.plan_severity {
                    // Severity escalation (e.g. Warning -> Critical):
                    // Reset to a fresh plan and attempt budget for this urgent notification!
                    rec.plan_hash = event_plan_hash;
                    rec.plan_severity = event.severity;
                    rec.chunks = fresh_chunks.clone();
                    rec.confirmed_chunks = 0;
                    rec.attempts = 0;
                    rec.exhausted = false;
                    rec.exhausted_at = None;
                    (rec.chunks.clone(), 0, 0, false)
                } else if rec.exhausted || rec.attempts >= 3 {
                    rec.exhausted = true;
                    (rec.chunks.clone(), 0, 3, true)
                } else if rec.plan_hash != event_plan_hash {
                    // Plan content changed at same severity:
                    rec.plan_hash = event_plan_hash;
                    rec.plan_severity = event.severity;
                    rec.chunks = fresh_chunks.clone();
                    rec.confirmed_chunks = 0;
                    (rec.chunks.clone(), 0, rec.attempts, false)
                } else {
                    // Identical plan: resume from confirmed chunks with stored immutable chunks!
                    (
                        rec.chunks.clone(),
                        rec.confirmed_chunks,
                        rec.attempts,
                        false,
                    )
                }
            } else {
                (fresh_chunks, 0, 0, false)
            }
        };

        if is_exhausted {
            debug!(
                "Skipping send: incident generation {:?} is already exhausted",
                inc_id
            );
            return Err(SentinelError::Action(
                "Delivery attempts exhausted for this incident generation".into(),
            ));
        }

        if starting_chunk >= plan_chunks.len() {
            return Ok(());
        }

        let mut confirmed_chunks = starting_chunk;
        let tg_url = format!("{}/bot{}/sendMessage", self.api_base, tg.bot_token);
        let mut last_err = None;

        for attempt in starting_attempt..3 {
            let mut send_err = None;

            while confirmed_chunks < plan_chunks.len() {
                self.wait_for_rate_limit(shutdown_rx).await?;

                let chunk = &plan_chunks[confirmed_chunks];
                let body = json!({
                    "chat_id": tg.chat_id,
                    "text": chunk,
                    "parse_mode": "HTML",
                    "disable_web_page_preview": true
                });

                let send_attempt = async {
                    let resp = self
                        .client
                        .post(&tg_url)
                        .json(&body)
                        .send()
                        .await
                        .map_err(|e| {
                            SentinelError::Action(format!(
                                "Network error sending Telegram message: {}",
                                e
                            ))
                        })?;
                    let status = resp.status();
                    if status.as_u16() == 429 {
                        let err_text = resp.text().await.unwrap_or_default();
                        return Ok(TelegramChunkOutcome::RateLimit(err_text));
                    }
                    if status.as_u16() == 400 {
                        let fallback_body = json!({
                            "chat_id": tg.chat_id,
                            "text": truncate_field(chunk, 4096),
                            "disable_web_page_preview": true
                        });
                        let fb_resp = self
                            .client
                            .post(&tg_url)
                            .json(&fallback_body)
                            .send()
                            .await
                            .map_err(|e| {
                                SentinelError::Action(format!(
                                    "Network error in Telegram fallback: {}",
                                    e
                                ))
                            })?;
                        let fb_status = fb_resp.status();
                        if fb_status.as_u16() == 429 {
                            let err_text = fb_resp.text().await.unwrap_or_default();
                            return Ok(TelegramChunkOutcome::FallbackRateLimit(err_text));
                        }
                        Self::check_telegram_response(fb_resp).await?;
                        return Ok(TelegramChunkOutcome::Success);
                    }
                    Self::check_telegram_response(resp).await?;
                    Ok(TelegramChunkOutcome::Success)
                };

                let outcome = match cancellable(send_attempt, shutdown_rx).await {
                    Ok(o) => o,
                    Err(e) => {
                        send_err = Some(e);
                        break;
                    }
                };

                match outcome {
                    TelegramChunkOutcome::Success => {
                        confirmed_chunks += 1;
                        let mut state = self.delivery_state.lock().unwrap();
                        if let Some(inc) = state.incidents.get_mut(&inc_id.key) {
                            let rec = inc.records.entry(inc_id.generation).or_default();
                            rec.confirmed_chunks = confirmed_chunks;
                        }
                    }
                    TelegramChunkOutcome::RateLimit(err_text) => {
                        let wait_sec = self.extract_retry_after(&err_text).unwrap_or(5);
                        self.record_rate_limit(wait_sec);
                        send_err = Some(SentinelError::Action(format!(
                            "Telegram API HTTP 429: {}",
                            err_text
                        )));
                        break;
                    }
                    TelegramChunkOutcome::FallbackRateLimit(err_text) => {
                        let wait_sec = self.extract_retry_after(&err_text).unwrap_or(5);
                        self.record_rate_limit(wait_sec);
                        send_err = Some(SentinelError::Action(format!(
                            "Telegram API fallback HTTP 429: {}",
                            err_text
                        )));
                        break;
                    }
                }
            }

            if confirmed_chunks == plan_chunks.len() {
                info!("Dispatched Telegram notification to chat '{}'", tg.chat_id);
                return Ok(());
            }

            // Record this attempt in state
            {
                let mut state = self.delivery_state.lock().unwrap();
                if let Some(inc) = state.incidents.get_mut(&inc_id.key) {
                    let rec = inc.records.entry(inc_id.generation).or_default();
                    rec.attempts = attempt + 1;
                    if rec.attempts >= 3 {
                        rec.exhausted = true;
                        rec.exhausted_at = Some(std::time::Instant::now());
                        debug!(
                            "Marked incident generation {:?} as exhausted at severity {:?}",
                            inc_id, rec.plan_severity
                        );
                    }
                }
            }

            last_err = send_err;
            if attempt + 1 < 3 {
                let sleep_fut = async {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    Ok(())
                };
                cancellable(sleep_fut, shutdown_rx).await?;
            }
        }

        Err(last_err.unwrap_or_else(|| {
            SentinelError::Action("Failed to deliver alert after retries".into())
        }))
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
        let event = self.build_alert_event(
            AlertSource::JevAdvisor,
            None,
            if decision.is_critical() {
                AlertSeverity::Critical
            } else if decision.is_warning() {
                AlertSeverity::Warning
            } else {
                AlertSeverity::Info
            },
            AlertReason::RiskElevated,
            format!("Jev System 1: {}", decision.system_health.to_uppercase()),
            &message_text,
        );
        self.send_alert_message(&event, &message_text, None).await?;
        self.commit_delivery(&event);
        Ok(())
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
        let event = self.build_alert_event(
            AlertSource::LocalRule,
            None,
            AlertSeverity::Info,
            AlertReason::RiskElevated,
            "Telegram Alert Test",
            &message_text,
        );
        self.send_alert_message(&event, &message_text, None).await
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
            escape_html(&sanitize_inline_field(
                &decision.system_health.to_uppercase(),
                40
            ))
        ));
        lines.push(format!(
            "<b>Risk score:</b> {:.2} / 1.00",
            decision.risk_score
        ));
        lines.push(format!(
            "<b>Suggested Action:</b> <code>{}</code>",
            escape_html(&sanitize_inline_field(&decision.suggested_action, 80))
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
                escape_html(&sanitize_inline_field(&target.target_name, 60)),
                escape_html(&sanitize_inline_field(&target.target_type, 30)),
                target.latency_ms
            ));
            if let Some(err) = &target.error_message {
                let truncated_err = sanitize_inline_field(err, 180);
                lines.push(format!(
                    "  <i>Error:</i> <code>{}</code>",
                    escape_html(&truncated_err)
                ));
            }
        }

        lines.join("\n")
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
    fn test_multiline_target_names_remain_single_line_in_html() {
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

        // 29 targets with names containing newlines: "node-i\nzzz"
        let targets = (0..29)
            .map(|i| TargetTelemetry {
                target_name: format!("node-{i}\nzzz"),
                target_type: "exec_probe".to_string(),
                status: TargetStatus::Degraded,
                latency_ms: 1.0,
                metrics: serde_json::json!({}),
                error_message: Some("x\n".to_string() + &"y".repeat(80)),
                timestamp: chrono::Utc::now(),
                observed_at: None,
            })
            .collect();

        let snapshot = InfrastructureSnapshot {
            timestamp: chrono::Utc::now(),
            targets,
        };

        let msg = notifier.format_decision_message(&decision, &snapshot);
        for line in msg.lines() {
            if line.contains("<b>") {
                assert!(
                    line.contains("</b>"),
                    "<b> tag must close on the same line: {}",
                    line
                );
            }
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

        let event = notifier.build_alert_event(
            AlertSource::LocalRule,
            Some("server-1".to_string()),
            AlertSeverity::Warning,
            AlertReason::ServiceDegraded,
            "Service Degraded".to_string(),
            "High latency detected".to_string(),
        );

        // 1. Initial enqueue succeeds
        assert!(notifier.enqueue_alert(event.clone()));

        // 2. Commit delivery
        assert!(notifier.commit_delivery(&event));

        // 3. Same severity within cooldown -> suppressed
        assert!(!notifier.enqueue_alert(event.clone()));

        // 4. Escalation to Critical -> not suppressed
        let crit_event = AlertEvent {
            severity: AlertSeverity::Critical,
            ..event.clone()
        };
        assert!(notifier.enqueue_alert(crit_event));

        // 5. Target recovery clears cooldown and cancels pending
        notifier.recover_target("server-1");
        // Next event has new generation and is admitted immediately
        let new_event = notifier.build_alert_event(
            AlertSource::LocalRule,
            Some("server-1".to_string()),
            AlertSeverity::Warning,
            AlertReason::ServiceDegraded,
            "Service Degraded".to_string(),
            "High latency detected".to_string(),
        );
        assert!(notifier.enqueue_alert(new_event));

        // 6. Advisor recovery clears both advisor unavailable AND risk elevated cooldown
        let adv_event = notifier.build_alert_event(
            AlertSource::LocalRule,
            None,
            AlertSeverity::Warning,
            AlertReason::AdvisorUnavailable,
            "Advisor Unavailable".to_string(),
            "Network timeout".to_string(),
        );
        assert!(notifier.enqueue_alert(adv_event.clone()));
        notifier.commit_delivery(&adv_event);
        assert!(!notifier.enqueue_alert(adv_event.clone())); // suppressed
        notifier.recover_advisor_unavailable();
        let fresh_adv = notifier.build_alert_event(
            AlertSource::LocalRule,
            None,
            AlertSeverity::Warning,
            AlertReason::AdvisorUnavailable,
            "Advisor Unavailable".to_string(),
            "Network timeout".to_string(),
        );
        assert!(notifier.enqueue_alert(fresh_adv)); // allowed after recovery
    }

    #[test]
    fn test_late_ack_race_does_not_suppress_new_incident() {
        let notifier = Notifier::new(Some(TelegramAlertSettings {
            bot_token: "fake-token".to_string(),
            chat_id: "123456".to_string(),
            min_severity: "warning".to_string(),
            proxy: None,
        }));

        let target_id = "node-A";

        // t0: Incident 1 begins. Generation is stamped onto event at creation
        let i1 = notifier.build_alert_event(
            AlertSource::LocalRule,
            Some(target_id.to_string()),
            AlertSeverity::Warning,
            AlertReason::ServiceCheckFailed,
            "Service Check Failed".to_string(),
            "Error 500".to_string(),
        );
        assert_eq!(i1.generation, 0);

        // t1: Target recovers while HTTP is in flight!
        notifier.recover_target(target_id);

        // t2: Late HTTP response for Incident 1 arrives.
        // commit_delivery sees that target's generation is now 1, discarding late ACK
        assert!(!notifier.commit_delivery(&i1));

        // t3: Target fails again (Incident 2)!
        let i2 = notifier.build_alert_event(
            AlertSource::LocalRule,
            Some(target_id.to_string()),
            AlertSeverity::Warning,
            AlertReason::ServiceCheckFailed,
            "Service Check Failed".to_string(),
            "Error 500".to_string(),
        );
        assert_eq!(i2.generation, 1);

        // Incident 2 must NOT be suppressed!
        assert!(
            notifier.enqueue_alert(i2),
            "Incident 2 must not be suppressed by late ACK of Incident 1"
        );
    }

    #[test]
    fn test_pending_coalescing_and_priority_admission() {
        let notifier = Notifier::new(Some(TelegramAlertSettings {
            bot_token: "fake-token".to_string(),
            chat_id: "123456".to_string(),
            min_severity: "warning".to_string(),
            proxy: None,
        }));

        let warn_event = notifier.build_alert_event(
            AlertSource::LocalRule,
            Some("node-A".to_string()),
            AlertSeverity::Warning,
            AlertReason::ServiceDegraded,
            "Degraded".to_string(),
            "Load high".to_string(),
        );

        // Enqueue 128 duplicates of the same event: they must coalesce into 1 pending entry!
        for _ in 0..128 {
            assert!(notifier.enqueue_alert(warn_event.clone()));
        }

        {
            let state = notifier.delivery_state.lock().unwrap();
            assert_eq!(
                state.pending.len(),
                1,
                "Duplicates must coalesce into 1 pending slot"
            );
        }

        // A new Critical event arrives: must be admitted and prioritized over Warning!
        let crit_event = notifier.build_alert_event(
            AlertSource::JevAdvisor,
            None,
            AlertSeverity::Critical,
            AlertReason::RiskElevated,
            "Critical Outage".to_string(),
            "Urgent action required".to_string(),
        );
        assert!(notifier.enqueue_alert(crit_event));

        // Next popped pending event must be the Critical one!
        let popped = notifier
            .pop_next_pending()
            .expect("Must have pending event");
        assert_eq!(popped.severity, AlertSeverity::Critical);
    }

    #[test]
    fn test_risk_elevated_recovery_and_repeated_critical() {
        let notifier = Notifier::new(Some(TelegramAlertSettings {
            bot_token: "fake-token".to_string(),
            chat_id: "123456".to_string(),
            min_severity: "warning".to_string(),
            proxy: None,
        }));

        let crit = notifier.build_alert_event(
            AlertSource::JevAdvisor,
            None,
            AlertSeverity::Critical,
            AlertReason::RiskElevated,
            "Critical".to_string(),
            "OOM imminent".to_string(),
        );

        // 1. First critical alert is admitted and committed
        assert!(notifier.enqueue_alert(crit.clone()));
        assert!(notifier.commit_delivery(&crit));

        // 2. Continuing critical condition at t=60: must be suppressed within 300s cooldown!
        assert!(!notifier.enqueue_alert(crit.clone()));

        // 3. Jev reports healthy (risk normalized): recover_risk_elevated clears the cooldown
        notifier.recover_risk_elevated();

        // 4. A new critical risk arrives: must be admitted immediately!
        let crit_fresh = notifier.build_alert_event(
            AlertSource::JevAdvisor,
            None,
            AlertSeverity::Critical,
            AlertReason::RiskElevated,
            "Critical".to_string(),
            "OOM imminent".to_string(),
        );
        assert!(notifier.enqueue_alert(crit_fresh));
    }
}
