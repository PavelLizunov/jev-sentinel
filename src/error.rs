use thiserror::Error;

#[derive(Error, Debug)]
pub enum SentinelError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("HTTP client error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML deserialization error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Collector error on target '{target}': {message}")]
    Collector { target: String, message: String },

    #[error("TypeSafe Jev API error: {0}")]
    JevApi(String),

    #[error("Action execution error: {0}")]
    Action(String),
}

pub type Result<T> = std::result::Result<T, SentinelError>;
