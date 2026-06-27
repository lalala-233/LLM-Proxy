use thiserror::Error;

/// Error when loading or parsing configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid config JSON: {0}")]
    Parse(#[from] serde_json::Error),
}
