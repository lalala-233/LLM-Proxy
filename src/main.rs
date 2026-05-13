mod cli;
mod config;
mod error;
mod proxy;

use crate::{
    cli::Cli,
    config::Config,
    proxy::{AppState, create_router},
};
use clap::Parser;
use reqwest::Client;
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::{info, warn};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let config = match Config::load(&cli.config) {
        Ok(cfg) => {
            info!("Loaded configuration from {}", cli.config);
            cfg
        }
        Err(e) => {
            warn!("Failed to load {} (reason: {e})", cli.config);
            info!("Use default configuration");
            Config::default()
        }
    };

    // ---- HTTP client ---------------------------------------------------
    let client = Client::builder()
        .timeout(Duration::from_secs(config.timeout))
        .build()
        .expect("Failed to build HTTP client");

    let state = AppState { client, config };

    // priority: CLI > PORT env > config > 8000 (default)
    let mut port = state.config.port;

    if let Ok(env_str) = std::env::var("PORT") {
        match env_str.parse::<u16>() {
            Ok(env_port) => {
                if env_port != port {
                    warn!("PORT env var ({env_port}) overrides config port ({port})");
                }
                port = env_port;
            }
            Err(_) => {
                warn!("PORT env var '{env_str}' is not a valid port number, ignoring");
            }
        }
    }

    if let Some(cli_port) = cli.port {
        if cli_port != port {
            warn!("--port ({cli_port}) overrides current port ({port})");
        }
        port = cli_port;
    }

    let addr = format!("0.0.0.0:{port}");

    info!("{}", "=".repeat(50));
    info!("LLM Proxy starting:   http://{addr}");
    info!(
        "Allowed models:       {}",
        state.config.allowed_models.join(", ")
    );
    info!("Upstream URL:         {}", state.config.upstream);
    info!("{}", "=".repeat(50));

    let app = create_router(state);

    // ---- Bind ----------------------------------------------------------

    let listener = TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind {addr}: {e}"));
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| panic!("Server error: {e}"));
}
