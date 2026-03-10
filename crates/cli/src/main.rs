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
    about = "Kondi — Code-mode MCP proxy.",
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

    /// Start the kondid daemon in the background
    ///
    /// If the daemon is already running this is a no-op. Other commands
    /// (list, import) start the daemon automatically when needed.
    ///
    /// Example:
    ///   kondi mcp
    ///   kondi mcp --http 8080
    Mcp {
        /// Also expose an HTTP MCP endpoint on this port
        #[arg(long, value_name = "PORT")]
        http: Option<u16>,
    },

    /// Stop the kondid daemon
    ///
    /// Example:
    ///   kondi stop
    Stop,
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
            if let Some(data) = resp.data {
                if let Some(servers) = data.as_array() {
                    for s in servers {
                        if let Some(name) = s.get("name").and_then(|v| v.as_str()) {
                            println!("  {name}");
                        }
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

        Commands::Mcp { http } => {
            // Start kondid as a background process
            start_daemon(http)?;
            println!("daemon started");
            Ok(())
        }

        Commands::Stop => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()?;
            if client.get(format!("{admin_url}/health")).send().await.is_err() {
                println!("daemon is not running");
                return Ok(());
            }
            admin_call(&client, &admin_url, AdminRequest::Shutdown, &token).await?;
            println!("daemon stopped");
            Ok(())
        }
    }
}

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

fn start_daemon(http: Option<u16>) -> Result<()> {
    let mut cmd = std::process::Command::new("kondid");
    if let Some(port) = http {
        cmd.arg("--http").arg(port.to_string());
    }
    cmd.spawn().context("failed to spawn kondid — ensure it is in $PATH")?;
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
