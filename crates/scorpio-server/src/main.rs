use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// scorpio-server — HTTP API surface for the scorpio analyst pipeline.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Bind address.
    #[arg(long, env = "SCORPIO_SERVER_HOST", default_value = "0.0.0.0")]
    host: String,

    /// Port to listen on.
    #[arg(long, env = "SCORPIO_SERVER_PORT", default_value_t = 8088)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .with_context(|| format!("invalid bind address {}:{}", args.host, args.port))?;

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    info!(%addr, "scorpio-server listening");

    axum::serve(listener, scorpio_server::app())
        .await
        .context("axum::serve returned an error")?;

    Ok(())
}
