use rmcp::model::Tool;
use serde::Serialize;

/// A tool with its owning server name attached.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    pub server: String,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Aggregated catalog of tools from all connected MCP servers.
#[derive(Debug, Default)]
pub struct Catalog {
    entries: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register all tools from a given server.
    pub fn add_server_tools(&mut self, server_name: &str, tools: Vec<Tool>) {
        for tool in tools {
            self.entries.push(CatalogEntry {
                server: server_name.to_string(),
                name: tool.name.to_string(),
                description: tool.description.as_deref().unwrap_or("").to_string(),
                input_schema: serde_json::to_value(&tool.input_schema).unwrap_or_default(),
            });
        }
    }

    /// Return all entries as a JSON array.
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(&self.entries).unwrap_or_default()
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    /// Generate TypeScript type declarations for all servers and their tools.
    pub fn type_declarations(&self) -> String {
        let mut servers: std::collections::BTreeMap<&str, Vec<&CatalogEntry>> =
            std::collections::BTreeMap::new();
        for entry in &self.entries {
            servers.entry(&entry.server).or_default().push(entry);
        }

        let mut out = String::new();

        out.push_str("declare const tools: Array<{ server: string; name: string; description: string; input_schema: any }>;\n\n");

        for (server, tools) in &servers {
            let js_name = server.replace('-', "_");
            if !is_valid_js_ident(&js_name) {
                continue;
            }

            out.push_str(&format!("declare const {js_name}: {{\n"));
            for tool in tools {
                let params_type = schema_to_ts_params(&tool.input_schema);
                let desc = tool.description.replace('\n', " ").replace("*/", "* /");
                if !desc.is_empty() {
                    out.push_str(&format!("  /** {desc} */\n"));
                }
                let name_str = if is_valid_js_ident(&tool.name) {
                    format!("{name}(params: {{ {params_type} }}): Promise<any>;", name = tool.name)
                } else {
                    format!(
                        "\"{name}\"(params: {{ {params_type} }}): Promise<any>;",
                        name = tool.name
                    )
                };
                out.push_str(&format!("  {name_str}\n"));
            }
            out.push_str("};\n\n");
        }

        out
    }

    pub fn summary(&self) -> String {
        let mut servers: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for entry in &self.entries {
            *servers.entry(&entry.server).or_default() += 1;
        }
        let parts: Vec<String> = servers
            .iter()
            .map(|(name, count)| format!("{name}: {count} tools"))
            .collect();
        format!("{} total tools ({})", self.entries.len(), parts.join(", "))
    }
}

fn schema_to_ts_params(schema: &serde_json::Value) -> String {
    let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) else {
        return String::new();
    };

    let required: Vec<&str> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut params = Vec::new();
    for (name, prop) in properties {
        let ts_type = json_type_to_ts(prop);
        let optional = if required.contains(&name.as_str()) { "" } else { "?" };
        let name_str = if is_valid_js_ident(name) {
            format!("{name}{optional}")
        } else {
            format!("\"{name}\"{optional}")
        };
        params.push(format!("{name_str}: {ts_type}"));
    }

    params.join("; ")
}

fn json_type_to_ts(schema: &serde_json::Value) -> String {
    if let Some(enum_vals) = schema.get("enum").and_then(|v| v.as_array()) {
        let literals: Vec<String> = enum_vals
            .iter()
            .map(|v| match v {
                serde_json::Value::String(s) => format!("\"{s}\""),
                other => other.to_string(),
            })
            .collect();
        return literals.join(" | ");
    }

    let type_str = schema.get("type").and_then(|v| v.as_str()).unwrap_or("any");

    match type_str {
        "string" => "string".to_string(),
        "number" | "integer" => "number".to_string(),
        "boolean" => "boolean".to_string(),
        "null" => "null".to_string(),
        "array" => {
            if let Some(items) = schema.get("items") {
                format!("{}[]", json_type_to_ts(items))
            } else {
                "any[]".to_string()
            }
        }
        "object" => {
            if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
                if props.is_empty() {
                    "Record<string, any>".to_string()
                } else {
                    let inner = schema_to_ts_params(schema);
                    format!("{{ {inner} }}")
                }
            } else {
                "Record<string, any>".to_string()
            }
        }
        _ => "any".to_string(),
    }
}

fn is_valid_js_ident(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' && first != '$' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(server: &str, name: &str, desc: &str, schema: serde_json::Value) -> CatalogEntry {
        CatalogEntry {
            server: server.to_string(),
            name: name.to_string(),
            description: desc.to_string(),
            input_schema: schema,
        }
    }

    #[test]
    fn test_catalog_from_tools_entry_fields() {
        let mut catalog = Catalog::new();
        catalog.entries = vec![
            make_entry("github", "create_issue", "Create a new issue", serde_json::json!({})),
            make_entry("slack", "send_message", "Send a message", serde_json::json!({})),
        ];
        assert_eq!(catalog.entries().len(), 2);
        assert_eq!(catalog.entries()[0].server, "github");
        assert_eq!(catalog.entries()[0].name, "create_issue");
    }

    #[test]
    fn test_type_declarations_basic() {
        let mut catalog = Catalog::new();
        catalog.entries = vec![make_entry(
            "my-server",
            "navigate",
            "Navigate to URL",
            serde_json::json!({
                "type": "object",
                "properties": { "url": {"type": "string"} },
                "required": ["url"]
            }),
        )];

        let decls = catalog.type_declarations();
        assert!(decls.contains("declare const my_server:"), "decls: {decls}");
        assert!(decls.contains("navigate(params:"), "decls: {decls}");
        assert!(decls.contains("url: string"), "decls: {decls}");
    }
}