use tauri::{AppHandle, Manager};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri_plugin_autostart::MacosLauncher;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // If a second instance is launched, focus the existing window
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .setup(|app| {
            // Register autostart (login item / LaunchAgent / systemd --user)
            #[cfg(desktop)]
            app.handle().plugin(
                tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None),
            )?;

            build_tray(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            daemon_status,
            daemon_start,
            daemon_stop,
            daemon_restart,
            mcp_list,
        ])
        // Hide window on close — keep running in tray
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error running kondi");
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};

    let open = MenuItemBuilder::with_id("open", "Open Kondi").build(app)?;
    let start = MenuItemBuilder::with_id("start", "Start daemon").build(app)?;
    let stop = MenuItemBuilder::with_id("stop", "Stop daemon").build(app)?;
    let restart = MenuItemBuilder::with_id("restart", "Restart daemon").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit Kondi").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&open)
        .separator()
        .item(&start)
        .item(&stop)
        .item(&restart)
        .separator()
        .item(&quit)
        .build()?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Kondi")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_tray_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                let app = tray.app_handle();
                show_main_window(app);
            }
        })
        .build(app)?;

    Ok(())
}

fn handle_tray_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main_window(app),
        "start" => {
            tauri::async_runtime::spawn(cmd_daemon_start(app.clone()));
        }
        "stop" => {
            tauri::async_runtime::spawn(cmd_daemon_stop(app.clone()));
        }
        "restart" => {
            tauri::async_runtime::spawn(cmd_daemon_restart(app.clone()));
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

// ── Tauri commands (called from frontend via invoke) ─────────────────────────

/// Returns daemon status JSON or an error string.
#[tauri::command]
async fn daemon_status() -> Result<serde_json::Value, String> {
    admin_post(serde_json::json!({"command": "status"}))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn daemon_start() -> Result<String, String> {
    spawn_kondid().map_err(|e| e.to_string())?;
    wait_for_daemon(10).await.map_err(|e| e.to_string())?;
    Ok("started".into())
}

#[tauri::command]
async fn daemon_stop() -> Result<String, String> {
    admin_post(serde_json::json!({"command": "shutdown"}))
        .await
        .map(|_| "stopped".into())
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn daemon_restart() -> Result<String, String> {
    let _ = admin_post(serde_json::json!({"command": "shutdown"})).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    spawn_kondid().map_err(|e| e.to_string())?;
    wait_for_daemon(10).await.map_err(|e| e.to_string())?;
    Ok("restarted".into())
}

#[tauri::command]
async fn mcp_list() -> Result<serde_json::Value, String> {
    admin_post(serde_json::json!({"command": "list_mcp"}))
        .await
        .map_err(|e| e.to_string())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn admin_url() -> String {
    // TODO: read port from config
    "http://127.0.0.1:7337".to_string()
}

async fn admin_post(body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;
    let resp = client
        .post(format!("{}/admin", admin_url()))
        .json(&body)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    Ok(resp)
}

fn spawn_kondid() -> anyhow::Result<()> {
    let kondid = find_kondid_binary();
    std::process::Command::new(kondid).spawn()?;
    Ok(())
}

fn find_kondid_binary() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        for name in &["kondid", "kondid.exe"] {
            let p = exe.with_file_name(name);
            if p.exists() {
                return p;
            }
        }
    }
    std::path::PathBuf::from("kondid")
}

async fn wait_for_daemon(tries: u32) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(300))
        .build()?;
    for _ in 0..tries {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if client.get(format!("{}/health", admin_url())).send().await.is_ok() {
            return Ok(());
        }
    }
    anyhow::bail!("daemon did not start in time")
}

async fn cmd_daemon_start(app: AppHandle) {
    if let Err(e) = daemon_start().await {
        tracing::warn!("tray start daemon error: {e}");
    }
    update_tray_status(&app).await;
}

async fn cmd_daemon_stop(app: AppHandle) {
    if let Err(e) = daemon_stop().await {
        tracing::warn!("tray stop daemon error: {e}");
    }
    update_tray_status(&app).await;
}

async fn cmd_daemon_restart(app: AppHandle) {
    if let Err(e) = daemon_restart().await {
        tracing::warn!("tray restart daemon error: {e}");
    }
    update_tray_status(&app).await;
}

/// Update the tray icon tooltip to reflect current daemon state.
async fn update_tray_status(app: &AppHandle) {
    let running = reqwest::get(format!("{}/health", admin_url())).await.is_ok();
    if let Some(tray) = app.tray_by_id("main") {
        let tooltip = if running { "Kondi — running" } else { "Kondi — stopped" };
        let _ = tray.set_tooltip(Some(tooltip));
    }
}
