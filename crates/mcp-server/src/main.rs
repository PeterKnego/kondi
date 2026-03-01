mod admin_api;
mod server;

use anyhow::Result;
use clap::Parser;
use kondi_core::client::ClientPool;
use kondi_core::config::Config;
use rmcp::transport::stdio;
use rmcp::ServiceExt;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "kondid", about = "Kondi MCP daemon")]
struct Args {
    /// Run MCP server over HTTP instead of stdio
    #[arg(long)]
    http: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let config = Config::load()?;

    info!(server_count = config.servers.len(), "connecting to upstream servers");

    let (pool, catalog) = ClientPool::connect(config.servers.clone()).await?;
    info!("{}", catalog.summary());

    let mcp_server = server::KondiServer::new(pool, catalog).await?;
    let state = mcp_server.state.clone();

    // Start admin API in background
    let admin_port = config.admin.port;
    let admin_token = config.admin.token.clone();
    tokio::spawn(async move {
        if let Err(e) = admin_api::start_admin_server(admin_port, admin_token, state).await {
            tracing::error!(error = %e, "admin API error");
        }
    });

    // Start MCP server
    if let Some(_port) = args.http {
        // HTTP MCP transport — for now, serve stdio and log a note
        // rmcp 0.16 doesn't expose a streamable HTTP server transport directly.
        // The daemon can be run with a reverse proxy or stdio mode.
        tracing::warn!("HTTP MCP mode not yet supported by rmcp 0.16; falling back to stdio");
        let service = mcp_server.serve(stdio()).await?;
        service.waiting().await?;
    } else {
        info!("starting MCP server on stdio");
        let service = mcp_server.serve(stdio()).await?;
        service.waiting().await?;
    }

    Ok(())
}
