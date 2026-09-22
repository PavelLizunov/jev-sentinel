use crate::error::{Result, SentinelError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentinelConfig {
    #[serde(default = "default_version")]
    pub version: String,
    pub jev: JevSettings,
    #[serde(default)]
    pub daemon: DaemonSettings,
    #[serde(default)]
    pub targets: Vec<TargetConfig>,
    #[serde(default)]
    pub alerting: AlertingSettings,
    #[serde(default)]
    pub self_healing: SelfHealingSettings,
    #[serde(default)]
    pub web: WebSettings,
}

fn default_version() -> String {
    "1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JevSettings {
    pub api_key: String,
    #[serde(default = "default_jev_model")]
    pub model: String,
    #[serde(default = "default_jev_url")]
    pub base_url: String,
    pub egress_proxy: Option<String>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_eval_normal_seconds")]
    pub eval_cadence_normal_seconds: u64,
    #[serde(default = "default_eval_alert_seconds")]
    pub eval_cadence_alert_seconds: u64,
    #[serde(default = "default_true")]
    pub adaptive_digest_enabled: bool,
}

fn default_jev_model() -> String {
    "jev-latest".to_string()
}

fn default_jev_url() -> String {
    "https://api.typesafe.ai".to_string()
}

fn default_timeout_seconds() -> u64 {
    15
}

fn default_eval_normal_seconds() -> u64 {
    120
}

fn default_eval_alert_seconds() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonSettings {
    #[serde(default = "default_interval")]
    pub interval_seconds: u64,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    pub log_level: Option<String>,
}

impl Default for DaemonSettings {
    fn default() -> Self {
        Self {
            interval_seconds: default_interval(),
            concurrency: default_concurrency(),
            log_level: Some("info".to_string()),
        }
    }
}

fn default_interval() -> u64 {
    60
}

fn default_concurrency() -> usize {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TargetConfig {
    HttpProbe {
        name: String,
        url: String,
        #[serde(default = "default_http_status")]
        expected_status: u16,
        #[serde(default = "default_probe_timeout")]
        timeout_seconds: u64,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
    TcpPing {
        name: String,
        host: String,
        port: u16,
        #[serde(default = "default_probe_timeout")]
        timeout_seconds: u64,
    },
    ExecProbe {
        name: String,
        command: Vec<String>,
        #[serde(default = "default_probe_timeout")]
        timeout_seconds: u64,
    },
    MacosSsh {
        name: String,
        host: String,
        user: String,
        ssh_key: Option<String>,
        port: Option<u16>,
        #[serde(default = "default_probe_timeout")]
        timeout_seconds: u64,
        #[serde(default)]
        check_services: Vec<String>,
    },
    NvidiaSsh {
        name: String,
        host: String,
        user: String,
        ssh_key: Option<String>,
        port: Option<u16>,
        #[serde(default = "default_probe_timeout")]
        timeout_seconds: u64,
    },
}

impl TargetConfig {
    pub fn name(&self) -> &str {
        match self {
            Self::HttpProbe { name, .. } => name,
            Self::TcpPing { name, .. } => name,
            Self::ExecProbe { name, .. } => name,
            Self::MacosSsh { name, .. } => name,
            Self::NvidiaSsh { name, .. } => name,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::HttpProbe { .. } => "http_probe",
            Self::TcpPing { .. } => "tcp_ping",
            Self::ExecProbe { .. } => "exec_probe",
            Self::MacosSsh { .. } => "macos_ssh",
            Self::NvidiaSsh { .. } => "nvidia_ssh",
        }
    }
}

fn default_http_status() -> u16 {
    200
}

fn default_probe_timeout() -> u64 {
    5
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlertingSettings {
    pub telegram: Option<TelegramAlertSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramAlertSettings {
    pub bot_token: String,
    pub chat_id: String,
    #[serde(default = "default_min_severity")]
    pub min_severity: String, // info, warning, critical
    #[serde(default)]
    pub proxy: Option<String>,
}

fn default_min_severity() -> String {
    "warning".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SelfHealingSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_services: Vec<String>,
    #[serde(default = "default_true")]
    pub require_confirmation: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_web_listen")]
    pub listen: String,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: default_web_listen(),
        }
    }
}

fn default_web_listen() -> String {
    "127.0.0.1:8088".to_string()
}

// ── Environment Variable Interpolation ──

pub fn interpolate_env_vars(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' {
            if let Some(&'{') = chars.peek() {
                chars.next(); // consume '{'
                let mut var_name = String::new();
                let mut default_val = None;

                while let Some(nc) = chars.next() {
                    if nc == '}' {
                        break;
                    } else if nc == ':' && chars.peek() == Some(&'-') {
                        chars.next(); // consume '-'
                        let mut def = String::new();
                        for dc in chars.by_ref() {
                            if dc == '}' {
                                break;
                            }
                            def.push(dc);
                        }
                        default_val = Some(def);
                        break;
                    } else {
                        var_name.push(nc);
                    }
                }

                let val = std::env::var(&var_name)
                    .ok()
                    .or(default_val)
                    .unwrap_or_default();
                output.push_str(&val);
                continue;
            }
        }
        output.push(c);
    }
    output
}

impl std::str::FromStr for SentinelConfig {
    type Err = SentinelError;

    fn from_str(content: &str) -> Result<Self> {
        let interpolated = interpolate_env_vars(content);
        debug!("Parsed interpolated configuration YAML");
        let config: Self = serde_yaml::from_str(&interpolated)?;
        if config.daemon.interval_seconds == 0 || config.daemon.concurrency == 0 {
            return Err(SentinelError::Config(
                "interval_seconds and concurrency must be positive".into(),
            ));
        }
        if config.targets.len() > 1024 {
            return Err(SentinelError::Config(
                "at most 1024 probes are supported".into(),
            ));
        }
        let mut names = std::collections::HashSet::new();
        for target in &config.targets {
            if target.name().trim().is_empty()
                || target.name().len() > 128
                || !names.insert(target.name())
            {
                return Err(SentinelError::Config(
                    "probe names must be unique, nonempty and at most 128 bytes".into(),
                ));
            }
        }
        Ok(config)
    }
}

impl SentinelConfig {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| SentinelError::Config(format!("Failed to read config file: {}", e)))?;
        content.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interpolate_env_vars_with_default() {
        let input = "proxy: \"${NON_EXISTENT_VAR:-http://10.0.0.2:8080}\"";
        let out = interpolate_env_vars(input);
        assert_eq!(out, "proxy: \"http://10.0.0.2:8080\"");
    }

    #[test]
    fn test_interpolate_env_vars_with_actual_env() {
        std::env::set_var("JEV_TEST_SENTINEL_KEY", "secret-test-key-12345");
        let input = "api_key: \"${JEV_TEST_SENTINEL_KEY}\"";
        let out = interpolate_env_vars(input);
        assert_eq!(out, "api_key: \"secret-test-key-12345\"");
        std::env::remove_var("JEV_TEST_SENTINEL_KEY");
    }

    #[test]
    fn rejects_ambiguous_probe_identity_and_invalid_schedule() {
        let base = "jev:\n  api_key: test\n";
        for daemon in ["interval_seconds: 0", "concurrency: 0"] {
            assert!(format!("{base}daemon:\n  {daemon}\n")
                .parse::<SentinelConfig>()
                .is_err());
        }
        let probe = "  - type: tcp_ping\n    name: same\n    host: localhost\n    port: 80\n";
        assert!(format!("{base}targets:\n{probe}{probe}")
            .parse::<SentinelConfig>()
            .is_err());
        assert!(format!("{base}targets:\n{probe}")
            .parse::<SentinelConfig>()
            .is_ok());
        assert!(format!(
            "{base}targets:\n{}",
            probe.replace("name: same", "name: ''")
        )
        .parse::<SentinelConfig>()
        .is_err());
    }

    #[test]
    fn test_parse_config_with_telegram_proxy() {
        let yaml = r#"
jev:
  api_key: "test-key"
alerting:
  telegram:
    bot_token: "test-token"
    chat_id: "123456"
    min_severity: "warning"
    proxy: "http://10.0.0.2:8080"
"#;
        let config: SentinelConfig = yaml.parse().expect("Failed to parse config");
        assert_eq!(config.jev.api_key, "test-key");
        let tg = config.alerting.telegram.expect("Telegram config missing");
        assert_eq!(tg.bot_token, "test-token");
        assert_eq!(tg.chat_id, "123456");
        assert_eq!(tg.min_severity, "warning");
        assert_eq!(tg.proxy.as_deref(), Some("http://10.0.0.2:8080"));
    }
}
