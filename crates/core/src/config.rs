use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Top-level configuration.
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    pub servers: HashMap<String, ServerConfig>,
    #[serde(default)]
    pub admin: AdminConfig,
}

/// Admin API configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdminConfig {
    /// Port for the admin HTTP API.
    pub port: u16,
    /// Optional bearer token for admin API auth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            port: 7337,
            token: None,
        }
    }
}

/// Configuration for a single upstream MCP server.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(tag = "transport")]
pub enum ServerConfig {
    #[serde(rename = "http")]
    Http {
        url: String,
        /// Bearer token (without "Bearer " prefix).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<String>,
        /// Custom HTTP headers sent with every request.
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        headers: HashMap<String, String>,
    },

    #[serde(rename = "sse")]
    Sse {
        url: String,
        /// Bearer token (without "Bearer " prefix).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<String>,
        /// Custom HTTP headers.
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        headers: HashMap<String, String>,
    },

    #[serde(rename = "stdio")]
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        env: HashMap<String, String>,
    },
}

impl Config {
    /// Load config from a specific path, falling back to defaults if the file doesn't exist.
    pub fn load_from(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {}", path.display()))?;

        toml::from_str(&content)
            .with_context(|| format!("failed to parse config from {}", path.display()))
    }

    /// Load config from default path, falling back to defaults if the file doesn't exist.
    pub fn load() -> Result<Self> {
        Self::load_from(&default_config_path()?)
    }

    /// Save config to disk, creating parent dirs as needed.
    pub fn save(&self) -> Result<()> {
        self.save_to(&default_config_path()?)
    }

    /// Save config to a specific path, creating parent dirs as needed.
    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        let content = toml::to_string_pretty(self)
            .context("failed to serialize config")?;

        std::fs::write(path, content)
            .with_context(|| format!("failed to write config to {}", path.display()))
    }

    pub fn add_server(&mut self, name: String, config: ServerConfig) {
        self.servers.insert(name, config);
    }

    pub fn remove_server(&mut self, name: &str) -> bool {
        self.servers.remove(name).is_some()
    }
}

pub fn default_config_path() -> Result<PathBuf> {
    let config_dir = config_dir().context("could not determine config directory")?;
    Ok(config_dir.join("kondi").join("config.toml"))
}

/// Path to the PID file: same dir as config, named `kondid.pid`.
pub fn pid_path() -> Result<PathBuf> {
    let config_dir = config_dir().context("could not determine config directory")?;
    Ok(config_dir.join("kondi").join("kondid.pid"))
}

/// Path to the runtime directory for lock files / sockets.
/// Uses the same dir as config for simplicity.
pub fn runtime_dir() -> Result<PathBuf> {
    let config_dir = config_dir().context("could not determine config directory")?;
    let dir = config_dir.join("kondi");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_roundtrip() {
        let config = Config {
            servers: HashMap::new(),
            admin: AdminConfig { port: 7337, token: None },
        };
        let toml_str = toml::to_string(&config).unwrap();
        let loaded: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(loaded.admin.port, 7337);
        assert!(loaded.admin.token.is_none());
    }

    #[test]
    fn test_config_with_servers() {
        let mut config = Config::default();
        config.add_server(
            "test".to_string(),
            ServerConfig::Http {
                url: "https://example.com".to_string(),
                auth: Some("token123".to_string()),
                headers: HashMap::new(),
            },
        );
        let toml_str = toml::to_string(&config).unwrap();
        let loaded: Config = toml::from_str(&toml_str).unwrap();
        assert!(loaded.servers.contains_key("test"));
    }

    #[test]
    fn test_admin_config_default_port() {
        let config = Config::default();
        assert_eq!(config.admin.port, 7337);
    }
}
