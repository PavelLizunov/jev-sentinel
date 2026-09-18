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
                        while let Some(dc) = chars.next() {
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

impl SentinelConfig {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| SentinelError::Config(format!("Failed to read config file: {}", e)))?;
        Self::from_str(&content)
    }

    pub fn from_str(content: &str) -> Result<Self> {
        let interpolated = interpolate_env_vars(content);
        debug!("Parsed interpolated configuration YAML");
        let config: Self = serde_yaml::from_str(&interpolated)?;
        Ok(config)
    }
}
