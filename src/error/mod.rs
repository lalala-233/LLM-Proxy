pub mod config;
pub mod proxy;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Failed to build HTTP client\nReqwest Error: {0}")]
    Client(#[from] reqwest::Error),
    #[error("Failed to bind {addr}: {e}")]
    Binding { addr: String, e: std::io::Error },
    #[error("Server error: {0}")]
    Server(std::io::Error),
}
