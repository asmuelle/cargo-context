pub mod server;
pub mod tools;

use anyhow::Result;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tools::CargoContextServer;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "cargo-context-mcp starting"
    );

    let service = CargoContextServer::default().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
