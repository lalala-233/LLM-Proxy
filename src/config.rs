use serde::Deserialize;
use std::fmt;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Default values — used when config.json is missing or a field is omitted
// ---------------------------------------------------------------------------

fn default_upstream() -> String {
    "https://api.siliconflow.cn/v1/chat/completions".to_string()
}

fn default_allowed_models() -> Vec<String> {
    ["Qwen/Qwen3-8B", "THUDM/GLM-4-9B-0414"]
        .map(std::string::ToString::to_string)
        .to_vec()
}

const fn default_timeout() -> u64 {
    60
}

const fn default_port() -> u16 {
    8000
}

// ---------------------------------------------------------------------------
// Config struct
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Upstream API endpoint for chat completions.
    #[serde(default = "default_upstream")]
    pub upstream: String,
    /// Models the proxy accepts from clients.
    #[serde(default = "default_allowed_models")]
    pub allowed_models: Vec<String>,
    /// Timeout in seconds for upstream HTTP requests.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    /// Port the proxy listens on.
    #[serde(default = "default_port")]
    pub port: u16,
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

/// Error when loading or parsing configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    Io(#[from] std::io::Error),
    Parse(#[from] serde_json::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "Failed to read config file: {e}"),
            Self::Parse(e) => write!(f, "Invalid config JSON: {e}"),
        }
    }
}

impl Config {
    /// Load configuration from a JSON file.
    pub fn load(path: &str) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&content)?;
        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            upstream: default_upstream(),
            allowed_models: default_allowed_models(),
            timeout: default_timeout(),
            port: default_port(),
        }
    }
}
