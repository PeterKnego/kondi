use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

/// Minimal JSON-RPC 2.0 client over newline-delimited JSON (MCP stdio wire format).
pub struct McpTestClient {
    writer: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl McpTestClient {
    /// Create a new client, performing the MCP initialize handshake.
    pub async fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        let mut client = Self {
            writer: stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        };

        // Send initialize request.
        let init_id = client.next_id;
        client.next_id += 1;
        client
            .send(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": init_id,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "0.1.0"}
                }
            }))
            .await;

        // Read the initialize response (discard contents).
        client.recv_response(init_id).await;

        // Send initialized notification (no id → not a request).
        client
            .send(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .await;

        client
    }

    async fn send(&mut self, msg: &Value) {
        let mut line = serde_json::to_string(msg).expect("serialize MCP message");
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("write to daemon stdin");
        self.writer.flush().await.expect("flush daemon stdin");
    }

    /// Read lines until we find a response whose `id` matches.
    /// Lines without an `id` (notifications) are skipped.
    async fn recv_response(&mut self, id: u64) -> Value {
        let expected_id = serde_json::json!(id);
        loop {
            let mut line = String::new();
            let n = self
                .reader
                .read_line(&mut line)
                .await
                .expect("read from daemon stdout");
            assert!(n > 0, "EOF reading from daemon stdout");

            let msg: Value =
                serde_json::from_str(line.trim()).expect("parse MCP message");

            if msg.get("id") == Some(&expected_id) {
                // Return the result (or null if absent).
                return msg.get("result").cloned().unwrap_or(Value::Null);
            }
            // Notification or different id — skip.
        }
    }

    /// Send `tools/list` and return the `tools` array.
    pub async fn list_tools(&mut self) -> Vec<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/list",
            "params": {}
        }))
        .await;
        let result = self.recv_response(id).await;
        result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default()
    }

    /// Send `tools/call` and return the full result object.
    pub async fn call_tool(&mut self, name: &str, args: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": args
            }
        }))
        .await;
        self.recv_response(id).await
    }
}
