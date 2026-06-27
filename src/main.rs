use clap::Parser;
use llm_proxy::cli::Cli;
use std::process::exit;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    if let Err(error) = Cli::parse().run().await {
        eprintln!("{error}");
        exit(1);
    }
}
