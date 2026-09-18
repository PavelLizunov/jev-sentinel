pub mod actions;
pub mod collectors;
pub mod config;
pub mod core;
pub mod error;

pub use config::{SentinelConfig, TargetConfig};
pub use core::{InfrastructureSnapshot, JevClient, SentinelDecision, TargetStatus, TargetTelemetry};
pub use error::{Result, SentinelError};
