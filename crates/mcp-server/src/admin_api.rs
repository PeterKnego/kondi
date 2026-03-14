use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use axum::{Router, routing};
use kondi_core::admin::{AdminImportSource, AdminRequest, AdminResponse};
use kondi_core::bm25::BM25Index;
use kondi_core::client::ClientPool;
use kondi_core::config::Config;
use kondi_core::import;
use kondi_core::sandbox::Sandbox;
use serde_json::json;
use tokio::sync::Mutex;
use tokio::sync::watch;
use tracing::info;

use crate::server::ServerState;

struct AdminState {
    state: Arc<Mutex<ServerState>>,
    token: Option<String>,
    started_at: Instant,
    shutdown_tx: watch::Sender<bool>,
}

pub async fn start_admin_server(
    port: u16,
    token: Option<String>,
    state: Arc<Mutex<ServerState>>,
    started_at: Instant,
    shutdown_tx: watch::Sender<bool>,
) -> anyhow::Result<()> {
    let admin_state = Arc::new(AdminState { state, token, started_at, shutdown_tx });

    let app = Router::new()
        .route("/admin", routing::post(handle_admin))
        .route("/health", routing::get(handle_health))
        .with_state(admin_state);

    let addr = format!("127.0.0.1:{port}");
    info!("admin API listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_health() -> Json<serde_json::Value> {
    Json(json!({"ok": true}))
}

async fn handle_admin(
    State(admin): State<Arc<AdminState>>,
    headers: HeaderMap,
    Json(req): Json<AdminRequest>,
) -> (StatusCode, Json<AdminResponse>) {
    // Check bearer token if configured
    if let Some(expected_token) = &admin.token {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| t.trim());

        if auth != Some(expected_token.as_str()) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(AdminResponse::err("unauthorized")),
            );
        }
    }

    let response = dispatch_request(req, &admin).await;
    (StatusCode::OK, Json(response))
}

async fn dispatch_request(req: AdminRequest, admin: &AdminState) -> AdminResponse {
    let state = &admin.state;
    match req {
        AdminRequest::Status => {
            let s = state.lock().await;
            let server_count = s
                .catalog
                .entries()
                .iter()
                .map(|e| e.server.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len();
            AdminResponse::ok_with_data(
                "daemon running".to_string(),
                json!({
                    "pid": std::process::id(),
                    "uptime_secs": admin.started_at.elapsed().as_secs(),
                    "version": env!("CARGO_PKG_VERSION"),
                    "server_count": server_count,
                }),
            )
        }

        AdminRequest::ListMcp => {
            let s = state.lock().await;
            let servers: Vec<serde_json::Value> = s
                .catalog
                .entries()
                .iter()
                .map(|e| e.server.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .map(|name| json!({"name": name}))
                .collect();
            AdminResponse::ok_with_data(
                format!("{} server(s) registered", servers.len()),
                json!(servers),
            )
        }

        AdminRequest::AddMcp { name, config } => {
            let result: anyhow::Result<AdminResponse> = async {
                let mut cfg = Config::load()?;
                cfg.add_server(name.clone(), config);
                cfg.save()?;

                let (pool, catalog) = ClientPool::connect(cfg.servers).await?;
                let catalog = Arc::new(catalog);
                let pool = Arc::new(pool);
                let bm25 = BM25Index::build(catalog.entries());
                let sandbox = Sandbox::new(pool, catalog.clone()).await?;

                let mut s = state.lock().await;
                s.sandbox = sandbox;
                s.catalog = catalog;
                s.bm25 = bm25;

                Ok(AdminResponse::ok(format!("added server '{name}'")))
            }
            .await;

            result.unwrap_or_else(|e| AdminResponse::err(format!("failed to add server: {e}")))
        }

        AdminRequest::RemoveMcp { name } => {
            let result: anyhow::Result<AdminResponse> = async {
                let mut cfg = Config::load()?;
                if !cfg.remove_server(&name) {
                    return Ok(AdminResponse::err(format!("server '{name}' not found")));
                }
                cfg.save()?;

                let (pool, catalog) = ClientPool::connect(cfg.servers).await?;
                let catalog = Arc::new(catalog);
                let pool = Arc::new(pool);
                let bm25 = BM25Index::build(catalog.entries());
                let sandbox = Sandbox::new(pool, catalog.clone()).await?;

                let mut s = state.lock().await;
                s.sandbox = sandbox;
                s.catalog = catalog;
                s.bm25 = bm25;

                Ok(AdminResponse::ok(format!("removed server '{name}'")))
            }
            .await;

            result
                .unwrap_or_else(|e| AdminResponse::err(format!("failed to remove server: {e}")))
        }

        AdminRequest::ImportConfig { source: AdminImportSource::Claude } => {
            let result: anyhow::Result<AdminResponse> = async {
                let imported = import::import_claude_config()?;
                let count = imported.len();

                let mut cfg = Config::load()?;
                for server in imported {
                    cfg.add_server(server.name, server.config);
                }
                cfg.save()?;

                let (pool, catalog) = ClientPool::connect(cfg.servers).await?;
                let catalog = Arc::new(catalog);
                let pool = Arc::new(pool);
                let bm25 = BM25Index::build(catalog.entries());
                let sandbox = Sandbox::new(pool, catalog.clone()).await?;

                let mut s = state.lock().await;
                s.sandbox = sandbox;
                s.catalog = catalog;
                s.bm25 = bm25;

                Ok(AdminResponse::ok(format!("imported {count} server(s) from Claude config")))
            }
            .await;

            result.unwrap_or_else(|e| AdminResponse::err(format!("import failed: {e}")))
        }

        AdminRequest::Shutdown => {
            let _ = admin.shutdown_tx.send(true);
            AdminResponse::ok("daemon shutting down")
        }
    }
}
