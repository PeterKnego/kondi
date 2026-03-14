mod admin_api;
mod server;

use std::time::Instant;

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

/// Guard that removes the PID file when dropped.
struct PidGuard {
    path: std::path::PathBuf,
}

impl PidGuard {
    fn write(path: std::path::PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, std::process::id().to_string())?;
        Ok(Self { path })
    }
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let config = Config::load()?;

    // Write PID file; guard removes it on exit.
    let pid_path = kondi_core::config::pid_path()?;
    let _pid_guard = PidGuard::write(pid_path)?;

    let started_at = Instant::now();

    info!(server_count = config.servers.len(), "connecting to upstream servers");

    let (pool, catalog) = ClientPool::connect(config.servers.clone()).await?;
    info!("{}", catalog.summary());

    let mcp_server = server::KondiServer::new(pool, catalog).await?;
    let state = mcp_server.state.clone();

    // Shutdown channel
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    // Start admin API in background
    let admin_port = config.admin.port;
    let admin_token = config.admin.token.clone();
    tokio::spawn(async move {
        if let Err(e) = admin_api::start_admin_server(
            admin_port,
            admin_token,
            state,
            started_at,
            shutdown_tx,
        )
        .await
        {
            tracing::error!(error = %e, "admin API error");
        }
    });

    // Start MCP server
    if args.http.is_some() {
        tracing::warn!("HTTP MCP mode not yet supported by rmcp 0.16; falling back to stdio");
    }

    info!("starting MCP server on stdio");
    let service = mcp_server.serve(stdio()).await?;

    // Wait for either MCP service to finish or shutdown signal
    tokio::select! {
        res = service.waiting() => {
            if let Err(e) = res {
                tracing::error!(error = %e, "MCP service error");
            }
        }
        _ = shutdown_rx.changed() => {
            info!("shutdown signal received");
        }
    }

    Ok(())
}
