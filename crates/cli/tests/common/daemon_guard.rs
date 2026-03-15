#![allow(deprecated)]

use std::path::PathBuf;

use assert_fs::TempDir;
use kondi_core::admin::{AdminRequest, AdminResponse};
use tokio::process::{Child, ChildStdin, ChildStdout};

/// Manages a live `kondid` daemon process for e2e testing.
///
/// Creates an isolated home directory with its own config, spawns the daemon
/// on a free port, and tears it down on drop.
pub struct DaemonGuard {
    pub tmp: TempDir,
    #[allow(dead_code)]
    pub admin_port: u16,
    pub admin_url: String,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    child: Child,
}

impl DaemonGuard {
    /// Spawn a fresh `kondid` daemon in an isolated temp directory.
    ///
    /// Picks a free OS-assigned port, writes a minimal config, and polls
    /// `/health` for up to 5 seconds before returning.
    pub async fn spawn() -> Self {
        // Allocate a free port by binding and immediately dropping.
        let port = {
            let listener =
                std::net::TcpListener::bind("127.0.0.1:0").expect("bind for port allocation");
            listener.local_addr().unwrap().port()
        };

        let tmp = TempDir::new().expect("TempDir");

        // Write minimal config pointing to the free port.
        let cfg_dir = tmp.path().join(".config/kondi");
        std::fs::create_dir_all(&cfg_dir).expect("create config dir");
        std::fs::write(
            cfg_dir.join("config.toml"),
            format!("[admin]\nport = {port}\n"),
        )
        .expect("write config");

        let kondid_bin = assert_cmd::cargo::cargo_bin("kondid");
        let mut child = tokio::process::Command::new(&kondid_bin)
            .env("HOME", tmp.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn kondid");

        let stdin = child.stdin.take().expect("kondid stdin");
        let stdout = child.stdout.take().expect("kondid stdout");

        let health_url = format!("http://127.0.0.1:{port}/health");
        let client = reqwest::Client::new();

        // Poll /health for up to 5 seconds.
        let deadline =
            tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            if let Ok(resp) = client.get(&health_url).send().await {
                if resp.status().is_success() {
                    break;
                }
            }
            if tokio::time::Instant::now() > deadline {
                panic!("kondid did not become healthy within 5 seconds on port {port}");
            }
        }

        DaemonGuard {
            tmp,
            admin_port: port,
            admin_url: format!("http://127.0.0.1:{port}/admin"),
            stdin: Some(stdin),
            stdout: Some(stdout),
            child,
        }
    }

    /// Send a request to the daemon's admin API and return the response.
    pub async fn admin(&self, req: AdminRequest) -> AdminResponse {
        reqwest::Client::new()
            .post(&self.admin_url)
            .json(&req)
            .send()
            .await
            .expect("admin HTTP request")
            .json::<AdminResponse>()
            .await
            .expect("deserialize AdminResponse")
    }

    /// Take the daemon's stdin/stdout handles for use with [`McpTestClient`].
    ///
    /// Can only be called once per guard.
    pub fn take_stdio(&mut self) -> (ChildStdin, ChildStdout) {
        (
            self.stdin.take().expect("stdio already taken"),
            self.stdout.take().expect("stdio already taken"),
        )
    }

    /// Path to the `mock-mcp-server` binary built by Cargo.
    pub fn mock_server_bin() -> PathBuf {
        assert_cmd::cargo::cargo_bin("mock-mcp-server")
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}
