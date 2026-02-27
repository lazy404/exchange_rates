mod ecb;
mod rates;
mod server;

use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use server::ExchangeRateServer;

#[tokio::main]
async fn main() -> Result<()> {
    // Log to stderr — stdout is used exclusively for MCP JSON-RPC messages
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("Starting exchange_rates MCP server");

    let service = ExchangeRateServer::default()
        .serve(stdio())
        .await
        .inspect_err(|e| tracing::error!("Server error: {e}"))?;

    service.waiting().await?;

    Ok(())
}
