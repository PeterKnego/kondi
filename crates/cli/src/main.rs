use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use kondi_core::admin::{AdminImportSource, AdminRequest, AdminResponse};
use kondi_core::config::{Config, ServerConfig};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "kondi",
    about = "Kondi — MCP proxy CLI",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Add an MCP server to the proxy
    ///
    /// The transport is inferred automatically: URLs starting with http:// or https://
    /// default to "http", everything else defaults to "stdio". Use --transport to override.
    ///
    /// Examples:
    ///   kondi add myserver https://api.example.com/mcp
    ///   kondi add --transport sse myserver https://api.example.com/sse
    ///   kondi add --transport stdio github -- npx -y @modelcontextprotocol/server-github
    ///   kondi add --auth "Bearer token123" myserver https://api.example.com/mcp
    ///   kondi add -H "X-Api-Key: abc" myserver https://api.example.com/mcp
    ///   kondi add -e API_KEY=secret myserver -- /usr/bin/myserver
    Add {
        /// Transport protocol: http, sse, or stdio (auto-detected if omitted)
        #[arg(short, long, value_name = "TRANSPORT")]
        transport: Option<String>,
        /// Authorization header value, e.g. "Bearer <token>"
        #[arg(short, long, value_name = "VALUE")]
        auth: Option<String>,
        /// Extra HTTP headers in "Key: Value" format (repeatable)
        #[arg(short = 'H', long = "header", value_name = "KEY:VALUE")]
        headers: Vec<String>,
        /// Environment variables for stdio servers in "KEY=VALUE" format (repeatable)
        #[arg(short, long = "env", value_name = "KEY=VALUE")]
        envs: Vec<String>,
        /// Name to register the server under
        name: String,
        /// URL for http/sse transports, or command + arguments for stdio
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "URL|CMD [ARGS]...")]
        args: Vec<String>,
    },

    /// Remove a configured MCP server
    ///
    /// Example:
    ///   kondi remove myserver
    Remove {
        /// Name of the server to remove
        name: String,
    },

    /// List all configured MCP servers
    ///
    /// Alias: kondi ls
    ///
    /// Example:
    ///   kondi list
    #[command(alias = "ls")]
    List,

    /// Import MCP servers from an external config file
    ///
    /// Supported sources:
    ///   claude  — imports from ~/.claude.json (Claude Desktop config)
    ///
    /// Example:
    ///   kondi import claude
    Import {
        /// Config source to import from
        #[arg(value_enum, value_name = "SOURCE")]
        source: ImportSourceArg,
    },

    /// Manage the kondid daemon
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },

    /// Start the kondid daemon in the background (alias for 'daemon start')
    #[command(hide = true)]
    Mcp {
        /// Also expose an HTTP MCP endpoint on this port
        #[arg(long, value_name = "PORT")]
        http: Option<u16>,
    },

    /// Stop the kondid daemon (alias for 'daemon stop')
    #[command(hide = true)]
    Stop,
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Start the kondid daemon in the background
    Start {
        /// Also expose an HTTP MCP endpoint on this port
        #[arg(long, value_name = "PORT")]
        http: Option<u16>,
    },
    /// Stop the kondid daemon
    Stop,
    /// Show daemon status
    Status,
    /// Restart the daemon (stop then start)
    Restart {
        /// Also expose an HTTP MCP endpoint on this port
        #[arg(long, value_name = "PORT")]
        http: Option<u16>,
    },
    /// Install kondid as a login item / system service
    Install,
    /// Remove the kondid login item / system service
    Uninstall,
}

#[derive(Clone, ValueEnum)]
enum ImportSourceArg {
    Claude,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let cfg = Config::load().unwrap_or_default();
    let admin_url = format!("http://127.0.0.1:{}", cfg.admin.port);
    let token = cfg.admin.token.clone();

    match cli.command {
        Commands::Add { transport, auth, headers, envs, name, args } => {
            cmd_add_direct(&name, transport, auth, headers, envs, &args)
        }

        Commands::Remove { name } => cmd_remove_direct(&name),

        Commands::List => {
            ensure_daemon_running(&admin_url).await?;
            let client = reqwest::Client::new();
            let resp = admin_call(&client, &admin_url, AdminRequest::ListMcp, &token).await?;
            println!("{}", resp.message);
            if let Some(servers) = resp.data.as_ref().and_then(|d| d.as_array()) {
                for s in servers {
                    if let Some(name) = s.get("name").and_then(|v| v.as_str()) {
                        println!("  {name}");
                    }
                }
            }
            Ok(())
        }

        Commands::Import { source } => {
            ensure_daemon_running(&admin_url).await?;
            let client = reqwest::Client::new();
            let admin_source = match source {
                ImportSourceArg::Claude => AdminImportSource::Claude,
            };
            let resp = admin_call(
                &client,
                &admin_url,
                AdminRequest::ImportConfig { source: admin_source },
                &token,
            )
            .await?;
            println!("{}", resp.message);
            Ok(())
        }

        // Hidden alias: kondi mcp
        Commands::Mcp { http } => {
            start_daemon(http)?;
            println!("daemon started");
            Ok(())
        }

        // Hidden alias: kondi stop
        Commands::Stop => cmd_daemon_stop(&admin_url, &token).await,

        Commands::Daemon { cmd } => match cmd {
            DaemonCmd::Start { http } => {
                start_daemon(http)?;
                println!("daemon started");
                Ok(())
            }
            DaemonCmd::Stop => cmd_daemon_stop(&admin_url, &token).await,
            DaemonCmd::Status => cmd_daemon_status(&admin_url, &token).await,
            DaemonCmd::Restart { http } => cmd_daemon_restart(&admin_url, &token, http).await,
            DaemonCmd::Install => cmd_daemon_install(),
            DaemonCmd::Uninstall => cmd_daemon_uninstall(),
        },
    }
}

// ── Daemon commands ─────────────────────────────────────────────────────────

async fn cmd_daemon_stop(admin_url: &str, token: &Option<String>) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    if client.get(format!("{admin_url}/health")).send().await.is_err() {
        println!("daemon is not running");
        return Ok(());
    }
    admin_call(&client, admin_url, AdminRequest::Shutdown, token).await?;
    println!("daemon stopped");
    Ok(())
}

async fn cmd_daemon_status(admin_url: &str, token: &Option<String>) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    if client.get(format!("{admin_url}/health")).send().await.is_err() {
        println!("kondid is not running");
        return Ok(());
    }
    let resp = admin_call(&client, admin_url, AdminRequest::Status, token).await?;
    println!("kondid is running");
    if let Some(data) = resp.data {
        if let Some(pid) = data.get("pid").and_then(|v| v.as_u64()) {
            println!("  PID:      {pid}");
        }
        if let Some(uptime) = data.get("uptime_secs").and_then(|v| v.as_u64()) {
            let mins = uptime / 60;
            let secs = uptime % 60;
            println!("  Uptime:   {mins}m {secs}s");
        }
        if let Some(version) = data.get("version").and_then(|v| v.as_str()) {
            println!("  Version:  {version}");
        }
        if let Some(count) = data.get("server_count").and_then(|v| v.as_u64()) {
            println!("  Servers:  {count} connected");
        }
    }
    Ok(())
}

async fn cmd_daemon_restart(admin_url: &str, token: &Option<String>, http: Option<u16>) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    if client.get(format!("{admin_url}/health")).send().await.is_ok() {
        admin_call(&client, admin_url, AdminRequest::Shutdown, token).await?;
        wait_for_daemon_down(admin_url).await?;
    }
    start_daemon(http)?;
    ensure_daemon_running(admin_url).await?;
    println!("daemon restarted");
    Ok(())
}

fn cmd_daemon_install() -> Result<()> {
    let kondid = find_kondid();
    platform::install_service(&kondid)
}

fn cmd_daemon_uninstall() -> Result<()> {
    platform::uninstall_service()
}

// ── Platform autostart (Phase 4) ────────────────────────────────────────────

/// Escape special XML characters in a string for safe embedding in plist/XML.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::Path;
    use anyhow::Result;
    use crate::xml_escape;

    const LABEL: &str = "app.kondi.daemon";
    const PLIST_PATH: &str = "Library/LaunchAgents/app.kondi.daemon.plist";

    pub fn install_service(kondid: &Path) -> Result<()> {
        let home = std::env::var("HOME").context("HOME not set")?;
        let plist_dir = format!("{home}/Library/LaunchAgents");
        std::fs::create_dir_all(&plist_dir)?;
        let plist_path = format!("{home}/{PLIST_PATH}");
        let kondid_escaped = xml_escape(&kondid.display().to_string());
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>             <string>{LABEL}</string>
  <key>ProgramArguments</key>  <array><string>{kondid_escaped}</string></array>
  <key>RunAtLoad</key>         <true/>
  <key>KeepAlive</key>         <false/>
  <key>StandardOutPath</key>   <string>/tmp/kondid.log</string>
  <key>StandardErrorPath</key> <string>/tmp/kondid.err</string>
</dict>
</plist>
"#
        );
        std::fs::write(&plist_path, plist)?;
        std::process::Command::new("launchctl")
            .args(["load", "-w", &plist_path])
            .status()?;
        println!("kondid installed as LaunchAgent: {plist_path}");
        Ok(())
    }

    pub fn uninstall_service() -> Result<()> {
        let home = std::env::var("HOME").context("HOME not set")?;
        let plist_path = format!("{home}/{PLIST_PATH}");
        if std::path::Path::new(&plist_path).exists() {
            std::process::Command::new("launchctl")
                .args(["unload", "-w", &plist_path])
                .status()?;
            std::fs::remove_file(&plist_path)?;
            println!("kondid LaunchAgent removed");
        } else {
            println!("kondid is not installed as a LaunchAgent");
        }
        Ok(())
    }

    use anyhow::Context as _;
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::Path;
    use anyhow::{Context as _, Result};

    const SERVICE_FILE: &str = ".config/systemd/user/kondid.service";

    pub fn install_service(kondid: &Path) -> Result<()> {
        let kondid_str = kondid.display().to_string();
        anyhow::ensure!(
            !kondid_str.contains('\n'),
            "kondid path contains a newline — installation aborted"
        );
        let home = std::env::var("HOME").context("HOME not set")?;
        let service_dir = format!("{home}/.config/systemd/user");
        std::fs::create_dir_all(&service_dir)?;
        let service_path = format!("{home}/{SERVICE_FILE}");
        let unit = format!(
            "[Unit]\nDescription=Kondi MCP daemon\nAfter=network.target\n\n\
             [Service]\nExecStart={kondid_str}\nRestart=on-failure\nRestartSec=5\n\n\
             [Install]\nWantedBy=default.target\n"
        );
        std::fs::write(&service_path, unit)?;
        std::process::Command::new("systemctl")
            .args(["--user", "enable", "--now", "kondid"])
            .status()?;
        println!("kondid installed as systemd user service: {service_path}");
        Ok(())
    }

    pub fn uninstall_service() -> Result<()> {
        let home = std::env::var("HOME").context("HOME not set")?;
        let service_path = format!("{home}/{SERVICE_FILE}");
        if std::path::Path::new(&service_path).exists() {
            std::process::Command::new("systemctl")
                .args(["--user", "disable", "--now", "kondid"])
                .status()?;
            std::fs::remove_file(&service_path)?;
            println!("kondid systemd service removed");
        } else {
            println!("kondid is not installed as a systemd service");
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::path::Path;
    use anyhow::{Context as _, Result};

    pub fn install_service(kondid: &Path) -> Result<()> {
        use winreg::RegKey;
        use winreg::enums::*;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run_key = hkcu.open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_SET_VALUE,
        ).context("failed to open registry Run key")?;
        run_key.set_value("Kondi", &kondid.to_string_lossy().as_ref())?;
        println!("kondid registered in Windows startup registry");
        Ok(())
    }

    pub fn uninstall_service() -> Result<()> {
        use winreg::RegKey;
        use winreg::enums::*;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let run_key = hkcu.open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_SET_VALUE,
        ).context("failed to open registry Run key")?;
        let _ = run_key.delete_value("Kondi");
        println!("kondid removed from Windows startup registry");
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod platform {
    use std::path::Path;
    use anyhow::Result;

    pub fn install_service(_kondid: &Path) -> Result<()> {
        anyhow::bail!("daemon install is not supported on this platform")
    }

    pub fn uninstall_service() -> Result<()> {
        anyhow::bail!("daemon uninstall is not supported on this platform")
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Add a server directly by modifying config (no daemon needed).
fn cmd_add_direct(
    name: &str,
    transport: Option<String>,
    auth: Option<String>,
    headers: Vec<String>,
    envs: Vec<String>,
    args: &[String],
) -> Result<()> {
    let server_config = parse_server_config(transport, auth, headers, envs, args)?;
    let mut cfg = Config::load()?;
    cfg.add_server(name.to_string(), server_config);
    cfg.save()?;
    println!("added server '{name}'");
    Ok(())
}

/// Remove a server directly by modifying config.
fn cmd_remove_direct(name: &str) -> Result<()> {
    let mut cfg = Config::load()?;
    if cfg.remove_server(name) {
        cfg.save()?;
        println!("removed server '{name}'");
    } else {
        println!("server '{name}' not found");
    }
    Ok(())
}

fn parse_server_config(
    transport: Option<String>,
    auth: Option<String>,
    headers: Vec<String>,
    envs: Vec<String>,
    args: &[String],
) -> Result<ServerConfig> {
    let transport = transport.unwrap_or_else(|| {
        if let Some(first) = args.first() {
            if first.starts_with("http://") || first.starts_with("https://") {
                "http".to_string()
            } else {
                "stdio".to_string()
            }
        } else {
            "http".to_string()
        }
    });

    // Strip leading "--" separators
    let args: Vec<&str> = args.iter().skip_while(|a| a.as_str() == "--").map(|s| s.as_str()).collect();

    match transport.as_str() {
        "http" => {
            let url = args.first().context("missing URL")?.to_string();
            Ok(ServerConfig::Http {
                url,
                auth,
                headers: parse_headers_vec(&headers),
            })
        }
        "sse" => {
            let url = args.first().context("missing URL")?.to_string();
            Ok(ServerConfig::Sse {
                url,
                auth,
                headers: parse_headers_vec(&headers),
            })
        }
        "stdio" => {
            let command = args.first().context("missing command")?.to_string();
            let cmd_args: Vec<String> = args[1..].iter().map(|s| s.to_string()).collect();
            Ok(ServerConfig::Stdio {
                command,
                args: cmd_args,
                env: parse_envs_vec(&envs),
            })
        }
        other => anyhow::bail!("unknown transport '{other}'. Use: http, sse, or stdio"),
    }
}

fn parse_headers_vec(raw: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for h in raw {
        if let Some((k, v)) = h.split_once(':') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

fn parse_envs_vec(raw: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for e in raw {
        if let Some((k, v)) = e.split_once('=') {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}

/// Check if daemon is running; start it if not.
async fn ensure_daemon_running(admin_url: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;

    if client.get(format!("{admin_url}/health")).send().await.is_ok() {
        return Ok(());
    }

    // Daemon not running — spawn it
    start_daemon(None)?;

    // Wait up to 3 seconds
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if client.get(format!("{admin_url}/health")).send().await.is_ok() {
            return Ok(());
        }
    }

    anyhow::bail!("kondid did not start in time — run `kondid` manually")
}

/// Poll until the daemon's health endpoint stops responding.
async fn wait_for_daemon_down(admin_url: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()?;
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if client.get(format!("{admin_url}/health")).send().await.is_err() {
            return Ok(());
        }
    }
    anyhow::bail!("daemon did not stop in time")
}

/// Find the `kondid` binary: prefer the sibling of the running executable.
fn find_kondid() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("kondid");
        if sibling.exists() {
            return sibling;
        }
        let sibling_exe = exe.with_file_name("kondid.exe");
        if sibling_exe.exists() {
            return sibling_exe;
        }
    }
    std::path::PathBuf::from("kondid")
}

fn start_daemon(http: Option<u16>) -> Result<()> {
    let kondid = find_kondid();
    let mut cmd = std::process::Command::new(&kondid);
    if let Some(port) = http {
        cmd.arg("--http").arg(port.to_string());
    }
    cmd.spawn()
        .with_context(|| format!("failed to spawn {} — ensure kondid is in $PATH or next to kondi", kondid.display()))?;
    Ok(())
}

/// Send a request to the Admin API and return the response.
async fn admin_call(
    client: &reqwest::Client,
    admin_url: &str,
    req: AdminRequest,
    token: &Option<String>,
) -> Result<AdminResponse> {
    let mut builder = client.post(format!("{admin_url}/admin")).json(&req);

    if let Some(t) = token {
        builder = builder.header("Authorization", format!("Bearer {t}"));
    }

    let resp = builder
        .send()
        .await
        .context("failed to reach admin API")?
        .json::<AdminResponse>()
        .await
        .context("failed to parse admin response")?;

    Ok(resp)
}
