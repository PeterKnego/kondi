use std::sync::Arc;

use kondi_core::bm25::BM25Index;
use kondi_core::catalog::Catalog;
use kondi_core::client::ClientPool;
use kondi_core::sandbox::Sandbox;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;

const DEFAULT_MAX_LENGTH: usize = 40_000;

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
struct SearchRequest {
    #[schemars(description = "BM25 text search query to find relevant MCP tools by keyword")]
    query: String,
    #[schemars(description = "Max results to return. Default: 20.")]
    #[serde(default)]
    top_k: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchCodeRequest {
    #[schemars(description = "TypeScript code to filter/explore the tools catalog. A typed `tools` array is available. Must return a value.")]
    code: String,
    #[schemars(description = "Max response length in characters. Default: 40000.")]
    #[serde(default)]
    max_length: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExecuteRequest {
    #[schemars(description = "TypeScript code to execute. Each connected server is a typed global object where every tool is an async function.")]
    code: String,
    #[schemars(description = "Max response length in characters. Default: 40000.")]
    #[serde(default)]
    max_length: Option<usize>,
}

pub struct ServerState {
    pub sandbox: Sandbox,
    pub catalog: Arc<Catalog>,
    pub bm25: BM25Index,
}

/// The Kondi MCP server exposing search, search_code, and execute tools.
#[derive(Clone)]
pub struct KondiServer {
    pub state: Arc<Mutex<ServerState>>,
    tool_router: ToolRouter<Self>,
}

impl KondiServer {
    pub async fn new(pool: ClientPool, catalog: Catalog) -> anyhow::Result<Self> {
        let catalog = Arc::new(catalog);
        let pool = Arc::new(pool);
        let bm25 = BM25Index::build(catalog.entries());
        let sandbox = Sandbox::new(pool, catalog.clone()).await?;

        Ok(Self {
            state: Arc::new(Mutex::new(ServerState { sandbox, catalog, bm25 })),
            tool_router: Self::tool_router(),
        })
    }
}

#[tool_router]
impl KondiServer {
    #[tool(
        name = "search",
        description = "Search MCP tools by text query using BM25 ranking. Returns the most relevant tools from all connected servers."
    )]
    async fn search(
        &self,
        Parameters(req): Parameters<SearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        let state = self.state.lock().await;
        let top_k = req.top_k.unwrap_or(20);
        let results = state.bm25.search(&req.query, top_k);

        let data: Vec<serde_json::Value> = results
            .into_iter()
            .map(|(entry, score)| {
                serde_json::json!({
                    "server": entry.server,
                    "name": entry.name,
                    "description": entry.description,
                    "score": score,
                })
            })
            .collect();

        let text = serde_json::to_string_pretty(&data).unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(
        name = "search_code",
        description = "Filter MCP tools by running TypeScript code against the catalog. A typed `tools` array is available with { server, name, description, input_schema } fields."
    )]
    async fn search_code(
        &self,
        Parameters(req): Parameters<SearchCodeRequest>,
    ) -> Result<CallToolResult, McpError> {
        let max_len = req.max_length.unwrap_or(DEFAULT_MAX_LENGTH);
        let state = self.state.lock().await;
        match state.sandbox.search_code(&req.code).await {
            Ok(result) => {
                let text = serde_json::to_string_pretty(&result).unwrap_or_default();
                Ok(CallToolResult::success(vec![Content::text(truncate_response(
                    text, max_len,
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "search_code error: {e}"
            ))])),
        }
    }

    #[tool(
        name = "execute",
        description = "Execute TypeScript code that calls tools across all connected MCP servers. Each server is a typed global object. Chain calls or use Promise.all for parallel execution."
    )]
    async fn execute(
        &self,
        Parameters(req): Parameters<ExecuteRequest>,
    ) -> Result<CallToolResult, McpError> {
        let max_len = req.max_length.unwrap_or(DEFAULT_MAX_LENGTH);
        let state = self.state.lock().await;
        match state.sandbox.execute(&req.code).await {
            Ok(result) => {
                let text = serde_json::to_string_pretty(&result).unwrap_or_default();
                Ok(CallToolResult::success(vec![Content::text(truncate_response(
                    text, max_len,
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "execute error: {e}"
            ))])),
        }
    }
}

fn truncate_response(text: String, max_len: usize) -> String {
    if max_len == 0 || text.len() <= max_len {
        return text;
    }
    let cut = text[..max_len].rfind('\n').unwrap_or(max_len);
    let truncated = &text[..cut];
    let remaining = text.len() - cut;
    format!(
        "{truncated}\n\n[truncated — {remaining} chars omitted. Use your code to extract only the data you need, or increase max_length.]"
    )
}

#[tool_handler]
impl ServerHandler for KondiServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Kondi MCP Proxy.\n\n\
                 Use `search` to find tools by keyword (BM25).\n\
                 Use `search_code` to filter tools with TypeScript code.\n\
                 Use `execute` to call tools across servers with TypeScript code.\n\n\
                 Each connected server is a typed object in `execute` with auto-generated type declarations."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
