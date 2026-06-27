use crate::{
    config::Config,
    error::Error::{self, Server},
    proxy::{AppState, create_router},
};
use clap::Parser;
use reqwest::Client;
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::{info, warn};

/// LLM Proxy - OpenAI-compatible proxy server that sits between your client and an upstream LLM API.
///
/// By default, the proxy looks for `config.json` in the current working directory.
///
/// If the file is missing, built-in defaults are used (SiliconFlow upstream, port 8000).
#[derive(Parser)]
#[command(name = "llm-proxy", version, about)]
pub struct Cli {
    /// Path to the JSON configuration file
    #[arg(short = 'c', long = "config", default_value = "config.json")]
    pub config: String,

    /// Port to listen on (overrides config and PORT env var)
    ///
    /// Priority: CLI > PORT env > config > 8000 (default)
    #[arg(short = 'p', long = "port")]
    pub port: Option<u16>,
}
impl Cli {
    /// Starts the LLM proxy server.
    ///
    /// This function loads configuration (falling back to defaults on error),
    /// builds an HTTP client, resolves the listening port via CLI flag, environment
    /// variable (`PORT`), config, or default `8000`, then binds and serves requests.
    ///
    /// # Errors
    /// Returns an error if:
    /// - the HTTP client cannot be built (`Client`), typically due to TLS or
    ///   system resource issues;
    /// - the TCP listener fails to bind to the resolved address (`Binding`);
    /// - the server runtime encounters an I/O error while serving (`Server`),
    ///   e.g., the listener is closed or a fatal network error occurs.
    pub async fn run(&self) -> Result<(), Error> {
        let config = match Config::load(&self.config) {
            Ok(cfg) => {
                info!("Loaded configuration from {}", self.config);
                cfg
            }
            Err(e) => {
                warn!("Failed to load {} (reason: {e})", self.config);
                info!("Use default configuration");
                Config::default()
            }
        };

        // ---- HTTP client ---------------------------------------------------
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout))
            .build()?;

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

        if let Some(cli_port) = self.port {
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
            .map_err(|e| Error::Binding { addr, e })?;
        axum::serve(listener, app).await.map_err(Server)
    }
}
