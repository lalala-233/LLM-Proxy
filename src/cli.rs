use clap::Parser;

/// OpenAI-compatible proxy server that sits between your client and an upstream LLM API.
///
/// By default, the proxy looks for `config.json` in the current working directory.
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
