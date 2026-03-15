#![allow(deprecated)]

mod common;

use std::collections::HashMap;

use common::{DaemonGuard, McpTestClient};
use kondi_core::admin::AdminRequest;
use kondi_core::config::ServerConfig;

// ---------------------------------------------------------------------------
// Test 1: fresh daemon exposes exactly the three meta-tools
// ---------------------------------------------------------------------------

#[tokio::test]
async fn daemon_starts_exposes_meta_tools() {
    let mut guard = DaemonGuard::spawn().await;
    let (stdin, stdout) = guard.take_stdio();
    let mut client = McpTestClient::new(stdin, stdout).await;

    let tools = client.list_tools().await;
    let names: Vec<&str> =
        tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert_eq!(names.len(), 3, "expected 3 meta-tools, got: {names:?}");
    assert!(names.contains(&"search"));
    assert!(names.contains(&"search_code"));
    assert!(names.contains(&"execute"));
}

// ---------------------------------------------------------------------------
// Test 2: adding a server makes its tools visible in search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_server_tools_appear_in_search() {
    let mut guard = DaemonGuard::spawn().await;

    // Add the mock server via the admin API.
    let mock_bin = DaemonGuard::mock_server_bin();
    let resp = guard
        .admin(AdminRequest::AddMcp {
            name: "testserver".to_string(),
            config: ServerConfig::Stdio {
                command: mock_bin.to_str().unwrap().to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        })
        .await;
    assert!(resp.ok, "AddMcp failed: {}", resp.message);

    // Connect MCP client and search for "echo".
    let (stdin, stdout) = guard.take_stdio();
    let mut client = McpTestClient::new(stdin, stdout).await;

    let result = client
        .call_tool("search", serde_json::json!({"query": "echo"}))
        .await;
    let text = result["content"][0]["text"].as_str().unwrap_or("");

    assert!(
        text.contains("echo"),
        "expected 'echo' in search results, got:\n{text}"
    );
    assert!(
        text.contains("testserver"),
        "expected 'testserver' in search results, got:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// Test 3: execute tool can call an upstream tool end-to-end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn execute_calls_upstream_tool() {
    let mut guard = DaemonGuard::spawn().await;

    let mock_bin = DaemonGuard::mock_server_bin();
    let resp = guard
        .admin(AdminRequest::AddMcp {
            name: "testserver".to_string(),
            config: ServerConfig::Stdio {
                command: mock_bin.to_str().unwrap().to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        })
        .await;
    assert!(resp.ok, "AddMcp failed: {}", resp.message);

    let (stdin, stdout) = guard.take_stdio();
    let mut client = McpTestClient::new(stdin, stdout).await;

    let result = client
        .call_tool(
            "execute",
            serde_json::json!({
                "code": "return await testserver.echo({ message: 'hello-e2e' })"
            }),
        )
        .await;
    let text = result["content"][0]["text"].as_str().unwrap_or("");

    assert!(
        text.contains("hello-e2e"),
        "expected 'hello-e2e' in execute result, got:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// Test 4: removing a server makes its tools disappear from the catalog
// ---------------------------------------------------------------------------

#[tokio::test]
async fn remove_server_catalog_updates() {
    let mut guard = DaemonGuard::spawn().await;

    let mock_bin = DaemonGuard::mock_server_bin();
    let resp = guard
        .admin(AdminRequest::AddMcp {
            name: "testserver".to_string(),
            config: ServerConfig::Stdio {
                command: mock_bin.to_str().unwrap().to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        })
        .await;
    assert!(resp.ok, "AddMcp failed: {}", resp.message);

    let (stdin, stdout) = guard.take_stdio();
    let mut client = McpTestClient::new(stdin, stdout).await;

    // The tool should appear before removal.
    let result = client
        .call_tool("search", serde_json::json!({"query": "echo"}))
        .await;
    let text_before = result["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text_before.contains("echo"),
        "expected 'echo' before removal, got:\n{text_before}"
    );

    // Remove via admin API.
    let resp = guard
        .admin(AdminRequest::RemoveMcp { name: "testserver".to_string() })
        .await;
    assert!(resp.ok, "RemoveMcp failed: {}", resp.message);

    // The tool should no longer appear.
    let result = client
        .call_tool("search", serde_json::json!({"query": "echo"}))
        .await;
    let text_after = result["content"][0]["text"].as_str().unwrap_or("");
    assert!(
        text_after == "[]" || !text_after.contains("testserver"),
        "expected 'testserver' to be gone after removal, got:\n{text_after}"
    );
}

// ---------------------------------------------------------------------------
// Test 5: search_code can filter the catalog with TypeScript
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_code_filters_catalog() {
    let mut guard = DaemonGuard::spawn().await;

    let mock_bin = DaemonGuard::mock_server_bin();
    let resp = guard
        .admin(AdminRequest::AddMcp {
            name: "testserver".to_string(),
            config: ServerConfig::Stdio {
                command: mock_bin.to_str().unwrap().to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        })
        .await;
    assert!(resp.ok, "AddMcp failed: {}", resp.message);

    let (stdin, stdout) = guard.take_stdio();
    let mut client = McpTestClient::new(stdin, stdout).await;

    let result = client
        .call_tool(
            "search_code",
            serde_json::json!({
                "code": "return tools.filter(t => t.server === 'testserver').map(t => t.name)"
            }),
        )
        .await;
    let text = result["content"][0]["text"].as_str().unwrap_or("");

    assert!(
        text.contains("\"echo\""),
        "expected 'echo' in search_code result, got:\n{text}"
    );
    assert!(
        text.contains("\"greet\""),
        "expected 'greet' in search_code result, got:\n{text}"
    );
}

// ---------------------------------------------------------------------------
// Test 6: `kondi add` writes config; admin reload makes tools visible
//
// This test documents a known architectural gap: `kondi add` writes the
// config file but does NOT notify the running daemon. The daemon must be
// explicitly reloaded via AddMcp (or a future hot-reload mechanism).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cli_add_command_writes_config_daemon_reloads() {
    let mut guard = DaemonGuard::spawn().await;
    let mock_bin = DaemonGuard::mock_server_bin();

    // Run `kondi add` as a subprocess with HOME pointing at the temp dir.
    // This writes the server entry to config.toml but does NOT notify kondid.
    let status = std::process::Command::new(assert_cmd::cargo::cargo_bin("kondi"))
        .env("HOME", guard.tmp.path())
        .args([
            "add",
            "--transport",
            "stdio",
            "testserver",
            "--",
            mock_bin.to_str().unwrap(),
        ])
        .status()
        .expect("spawn kondi add");
    assert!(status.success(), "kondi add exited with: {status}");

    // Trigger daemon reload via AddMcp.  The admin handler reads config.toml
    // (which kondi add already wrote), re-connects, and atomically swaps state.
    let resp = guard
        .admin(AdminRequest::AddMcp {
            name: "testserver".to_string(),
            config: ServerConfig::Stdio {
                command: mock_bin.to_str().unwrap().to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        })
        .await;
    assert!(resp.ok, "AddMcp reload failed: {}", resp.message);

    // Verify via MCP search that the tools are now visible.
    let (stdin, stdout) = guard.take_stdio();
    let mut client = McpTestClient::new(stdin, stdout).await;

    let result = client
        .call_tool("search", serde_json::json!({"query": "echo"}))
        .await;
    let text = result["content"][0]["text"].as_str().unwrap_or("");

    assert!(
        text.contains("testserver"),
        "expected 'testserver' in search results after reload, got:\n{text}"
    );
}
