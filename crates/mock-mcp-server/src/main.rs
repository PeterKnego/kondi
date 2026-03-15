use clap::Parser;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::transport::stdio;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Parser)]
#[command(name = "mock-mcp-server", about = "Mock MCP server for e2e testing")]
struct Args {
    /// Restrict which tools are advertised (repeatable; no flags = all tools)
    #[arg(long = "tool")]
    _tools: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EchoRequest {
    #[schemars(description = "Message to echo back")]
    message: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GreetRequest {
    #[schemars(description = "Name of person to greet")]
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddRequest {
    #[schemars(description = "First number")]
    a: f64,
    #[schemars(description = "Second number")]
    b: f64,
}

#[derive(Clone)]
struct MockServer {
    tool_router: ToolRouter<Self>,
}

impl MockServer {
    fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl MockServer {
    #[tool(name = "echo", description = "Echo the message back")]
    async fn echo(
        &self,
        Parameters(req): Parameters<EchoRequest>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(req.message)]))
    }

    #[tool(name = "greet", description = "Greet a person by name")]
    async fn greet(
        &self,
        Parameters(req): Parameters<GreetRequest>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Hello, {}!",
            req.name
        ))]))
    }

    #[tool(name = "add", description = "Add two numbers together")]
    async fn add(
        &self,
        Parameters(req): Parameters<AddRequest>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(
            (req.a + req.b).to_string(),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for MockServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _args = Args::parse();
    let server = MockServer::new();
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
