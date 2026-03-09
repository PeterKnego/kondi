use assert_cmd::Command;
use assert_fs::TempDir;
use predicates::prelude::*;

#[allow(deprecated)]
fn kondi(tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("kondi").unwrap();
    cmd.env("HOME", tmp.path());
    cmd
}

#[test]
fn help_shows_usage() {
    let tmp = TempDir::new().unwrap();
    kondi(&tmp)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("MCP proxy CLI"));
}

#[test]
fn version_flag() {
    let tmp = TempDir::new().unwrap();
    kondi(&tmp)
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("kondi"));
}

#[test]
fn add_http_server_auto_detected() {
    let tmp = TempDir::new().unwrap();
    kondi(&tmp)
        .args(["add", "myserver", "https://api.example.com/mcp"])
        .assert()
        .success()
        .stdout(predicate::str::contains("added server"));

    let config = std::fs::read_to_string(
        tmp.path().join(".config/kondi/config.toml"),
    )
    .unwrap();
    assert!(config.contains("myserver"));
    assert!(config.contains("https://api.example.com/mcp"));
}

#[test]
fn add_stdio_server() {
    let tmp = TempDir::new().unwrap();
    kondi(&tmp)
        .args(["add", "--transport", "stdio", "ghserver", "--", "npx", "-y", "@modelcontextprotocol/server-github"])
        .assert()
        .success()
        .stdout(predicate::str::contains("added server"));

    let config = std::fs::read_to_string(
        tmp.path().join(".config/kondi/config.toml"),
    )
    .unwrap();
    assert!(config.contains("stdio"));
    assert!(config.contains("npx"));
}

#[test]
fn add_server_with_auth() {
    let tmp = TempDir::new().unwrap();
    kondi(&tmp)
        .args(["add", "--auth", "Bearer token123", "authserver", "https://api.example.com/mcp"])
        .assert()
        .success();

    let config = std::fs::read_to_string(
        tmp.path().join(".config/kondi/config.toml"),
    )
    .unwrap();
    assert!(config.contains("token123"));
}

#[test]
fn add_server_with_custom_header() {
    let tmp = TempDir::new().unwrap();
    kondi(&tmp)
        .args(["add", "-H", "X-Api-Key: abc", "hdrserver", "https://api.example.com/mcp"])
        .assert()
        .success();

    let config = std::fs::read_to_string(
        tmp.path().join(".config/kondi/config.toml"),
    )
    .unwrap();
    assert!(config.contains("abc"));
}

#[test]
fn add_multiple_servers() {
    let tmp = TempDir::new().unwrap();
    kondi(&tmp)
        .args(["add", "server1", "https://one.example.com/mcp"])
        .assert()
        .success();
    kondi(&tmp)
        .args(["add", "server2", "https://two.example.com/mcp"])
        .assert()
        .success();

    let config = std::fs::read_to_string(
        tmp.path().join(".config/kondi/config.toml"),
    )
    .unwrap();
    assert!(config.contains("server1"));
    assert!(config.contains("server2"));
}

#[test]
fn remove_existing_server() {
    let tmp = TempDir::new().unwrap();
    kondi(&tmp)
        .args(["add", "toremove", "https://api.example.com/mcp"])
        .assert()
        .success();

    kondi(&tmp)
        .args(["remove", "toremove"])
        .assert()
        .success()
        .stdout(predicate::str::contains("removed server"));
}

#[test]
fn remove_nonexistent_server() {
    let tmp = TempDir::new().unwrap();
    kondi(&tmp)
        .args(["remove", "ghost"])
        .assert()
        .success()
        .stdout(predicate::str::contains("server 'ghost' not found"));
}

#[test]
fn add_unknown_transport_fails() {
    let tmp = TempDir::new().unwrap();
    kondi(&tmp)
        .args(["add", "--transport", "grpc", "badserver", "https://api.example.com/mcp"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown transport"));
}
