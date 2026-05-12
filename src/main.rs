use std::time::Duration;

use reqwest::Client;
use tokio::net::TcpListener;
use tracing::info;

mod config;
mod error;
mod proxy;

use crate::config::{ALLOWED_MODELS, TIMEOUT_SECS, UPSTREAM_URL};
use crate::proxy::{AppState, create_router};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let client = Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .build()
        .expect("Failed to build HTTP client");

    let state = AppState { client };
    let app = create_router(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "8000".to_string());
    let addr = format!("0.0.0.0:{port}");

    info!("{}", "=".repeat(50));
    info!("LLM Proxy starting:   http://{}", addr);
    info!("Allowed models:       {}", ALLOWED_MODELS.join(", "));
    info!("Upstream URL:         {}", UPSTREAM_URL);
    info!("{}", "=".repeat(50));

    let listener = TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind {addr}: {e}"));
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| panic!("Server error: {e}"));
}
