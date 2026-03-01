use std::sync::Arc;

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
use tracing::info;

use crate::server::ServerState;

struct AdminState {
    state: Arc<Mutex<ServerState>>,
    token: Option<String>,
}

pub async fn start_admin_server(
    port: u16,
    token: Option<String>,
    state: Arc<Mutex<ServerState>>,
) -> anyhow::Result<()> {
    let admin_state = Arc::new(AdminState { state, token });

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

    let response = dispatch_request(req, &admin.state).await;
    (StatusCode::OK, Json(response))
}

async fn dispatch_request(req: AdminRequest, state: &Arc<Mutex<ServerState>>) -> AdminResponse {
    match req {
        AdminRequest::ListMcp => {
            let state = state.lock().await;
            let servers: Vec<serde_json::Value> = state
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
            // Load current config, add server, save, reconnect
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

        AdminRequest::Install => install_service(),
        AdminRequest::Uninstall => uninstall_service(),
    }
}

fn install_service() -> AdminResponse {
    let bin_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return AdminResponse::err(format!("could not determine binary path: {e}")),
    };

    #[cfg(target_os = "macos")]
    {
        let plist_content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>io.kondi.daemon</string>
  <key>ProgramArguments</key>
  <array><string>{bin}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/tmp/kondid.log</string>
</dict>
</plist>
"#,
            bin = bin_path.display()
        );

        let home = match std::env::var_os("HOME") {
            Some(h) => std::path::PathBuf::from(h),
            None => return AdminResponse::err("HOME not set"),
        };

        let plist_dir = home.join(".config").join("kondi");
        let plist_path = plist_dir.join("kondid.plist");

        if let Err(e) = std::fs::create_dir_all(&plist_dir) {
            return AdminResponse::err(format!("failed to create dir: {e}"));
        }
        if let Err(e) = std::fs::write(&plist_path, plist_content) {
            return AdminResponse::err(format!("failed to write plist: {e}"));
        }

        let status = std::process::Command::new("launchctl")
            .args(["load", "-w", &plist_path.to_string_lossy()])
            .status();

        match status {
            Ok(s) if s.success() => {
                AdminResponse::ok(format!("installed and loaded launchd service: {}", plist_path.display()))
            }
            _ => AdminResponse::ok(format!(
                "plist written to {}. Run manually: launchctl load -w {}",
                plist_path.display(),
                plist_path.display()
            )),
        }
    }

    #[cfg(target_os = "linux")]
    {
        let unit_content = format!(
            "[Unit]\nDescription=Kondi MCP Daemon\nAfter=network.target\n\n[Service]\nExecStart={bin}\nRestart=always\n\n[Install]\nWantedBy=default.target\n",
            bin = bin_path.display()
        );

        let home = match std::env::var_os("HOME") {
            Some(h) => std::path::PathBuf::from(h),
            None => return AdminResponse::err("HOME not set"),
        };

        let unit_dir = home.join(".config").join("systemd").join("user");
        let unit_path = unit_dir.join("kondid.service");

        if let Err(e) = std::fs::create_dir_all(&unit_dir) {
            return AdminResponse::err(format!("failed to create dir: {e}"));
        }
        if let Err(e) = std::fs::write(&unit_path, unit_content) {
            return AdminResponse::err(format!("failed to write unit file: {e}"));
        }

        AdminResponse::ok(format!(
            "systemd unit written to {}. Enable with: systemctl --user enable --now kondid",
            unit_path.display()
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = bin_path;
        AdminResponse::err("install not supported on this platform")
    }
}

fn uninstall_service() -> AdminResponse {
    #[cfg(target_os = "macos")]
    {
        let home = match std::env::var_os("HOME") {
            Some(h) => std::path::PathBuf::from(h),
            None => return AdminResponse::err("HOME not set"),
        };

        let plist_path = home.join(".config").join("kondi").join("kondid.plist");

        if plist_path.exists() {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", "-w", &plist_path.to_string_lossy()])
                .status();

            if let Err(e) = std::fs::remove_file(&plist_path) {
                return AdminResponse::err(format!("failed to remove plist: {e}"));
            }
        }

        AdminResponse::ok("uninstalled kondid service")
    }

    #[cfg(target_os = "linux")]
    {
        let home = match std::env::var_os("HOME") {
            Some(h) => std::path::PathBuf::from(h),
            None => return AdminResponse::err("HOME not set"),
        };

        let unit_path = home.join(".config").join("systemd").join("user").join("kondid.service");

        if unit_path.exists() {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "disable", "--now", "kondid"])
                .status();
            let _ = std::fs::remove_file(&unit_path);
        }

        AdminResponse::ok("uninstalled kondid service")
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        AdminResponse::err("uninstall not supported on this platform")
    }
}
