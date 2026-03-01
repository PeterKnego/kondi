use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::config::ServerConfig;

/// A discovered MCP server from an external source.
#[derive(Debug)]
pub struct ImportedServer {
    pub name: String,
    pub config: ServerConfig,
    pub source: ImportSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSource {
    ClaudeCode,
}

impl std::fmt::Display for ImportSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportSource::ClaudeCode => write!(f, "claude"),
        }
    }
}

/// Scan Claude config locations and return discovered servers.
pub fn import_claude_config() -> Result<Vec<ImportedServer>> {
    discover_claude_code()
}

fn discover_claude_code() -> Result<Vec<ImportedServer>> {
    let mut servers = Vec::new();
    let home = home_dir()?;

    // User-scoped: ~/.claude.json
    let user_config = home.join(".claude.json");
    if user_config.exists() {
        servers.extend(parse_claude_code_json(&user_config)?);
    }

    // Project-scoped: .mcp.json (current directory)
    let project_config = PathBuf::from(".mcp.json");
    if project_config.exists() {
        servers.extend(parse_claude_code_json(&project_config)?);
    }

    Ok(servers)
}

fn parse_claude_code_json(path: &PathBuf) -> Result<Vec<ImportedServer>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let root: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let Some(mcp_servers) = root.get("mcpServers").and_then(|v| v.as_object()) else {
        return Ok(Vec::new());
    };

    let mut servers = Vec::new();

    for (name, value) in mcp_servers {
        match parse_claude_code_server(name, value) {
            Ok(Some(server)) => servers.push(server),
            Ok(None) => {}
            Err(e) => {
                eprintln!("  warning: skipping {name}: {e}");
            }
        }
    }

    Ok(servers)
}

fn parse_claude_code_server(
    name: &str,
    value: &serde_json::Value,
) -> Result<Option<ImportedServer>> {
    let obj = value.as_object().context("server config is not an object")?;

    let transport = obj.get("type").and_then(|v| v.as_str()).unwrap_or("stdio");

    let config = match transport {
        "stdio" => {
            let command = obj
                .get("command")
                .and_then(|v| v.as_str())
                .context("missing command")?
                .to_string();

            let args = obj
                .get("args")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let env = parse_json_string_map(obj.get("env"));

            ServerConfig::Stdio { command, args, env }
        }
        "http" => {
            let url = obj
                .get("url")
                .and_then(|v| v.as_str())
                .context("missing url")?
                .to_string();

            let headers = parse_json_string_map(obj.get("headers"));
            let (auth, headers) = extract_auth_header(headers);

            ServerConfig::Http { url, auth, headers }
        }
        "sse" => {
            let url = obj
                .get("url")
                .and_then(|v| v.as_str())
                .context("missing url")?
                .to_string();

            let headers = parse_json_string_map(obj.get("headers"));
            let (auth, headers) = extract_auth_header(headers);

            ServerConfig::Sse { url, auth, headers }
        }
        // Skip internal types
        _ => return Ok(None),
    };

    Ok(Some(ImportedServer {
        name: name.to_string(),
        config,
        source: ImportSource::ClaudeCode,
    }))
}

fn parse_json_string_map(value: Option<&serde_json::Value>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(obj) = value.and_then(|v| v.as_object()) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                map.insert(k.clone(), s.to_string());
            }
        }
    }
    map
}

fn extract_auth_header(
    mut headers: HashMap<String, String>,
) -> (Option<String>, HashMap<String, String>) {
    let auth = headers
        .remove("Authorization")
        .or_else(|| headers.remove("authorization"))
        .and_then(|v| {
            if let Some(token) = v.strip_prefix("Bearer ") {
                Some(token.to_string())
            } else if let Some(token) = v.strip_prefix("bearer ") {
                Some(token.to_string())
            } else {
                headers.insert("Authorization".to_string(), v);
                None
            }
        });
    (auth, headers)
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).context("HOME not set")
}
