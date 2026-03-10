use serde::{Deserialize, Serialize};

use crate::config::ServerConfig;

/// Requests sent to the Admin API.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum AdminRequest {
    AddMcp { name: String, config: ServerConfig },
    RemoveMcp { name: String },
    ListMcp,
    ImportConfig { source: AdminImportSource },
    Shutdown,
}

/// Import source for the admin API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminImportSource {
    Claude,
}

/// Response from the Admin API.
#[derive(Debug, Serialize, Deserialize)]
pub struct AdminResponse {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl AdminResponse {
    pub fn ok(message: impl Into<String>) -> Self {
        Self { ok: true, message: message.into(), data: None }
    }

    pub fn ok_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self { ok: true, message: message.into(), data: Some(data) }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self { ok: false, message: message.into(), data: None }
    }
}
