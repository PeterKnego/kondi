# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
make build           # cargo build --workspace
make test            # all tests (unit + integration)
make test-unit       # cargo test --workspace --lib
make test-integration # cargo test -p kondi --test cli
make check           # cargo check --workspace
make fmt             # cargo fmt --all
make lint            # cargo clippy --workspace --all-targets -- -D warnings
```

Run a single test: `cargo test -p <crate> <test_name>` (e.g. `cargo test -p kondi-core test_execute_basic`)

## Architecture

Kondi is an **MCP proxy** — it aggregates multiple upstream MCP servers and exposes them as a single MCP server with three meta-tools: `search`, `search_code`, and `execute`.

### Two binaries

- **`kondid`** (`crates/mcp-server`) — the long-running daemon. Runs an MCP server over stdio. Also spawns an HTTP admin API on `127.0.0.1:7337`.
- **`kondi`** (`crates/cli`) — the user-facing CLI. Most commands communicate with the daemon via the admin API, but `add` and `remove` modify config directly without needing the daemon.

### Core library (`crates/core`)

- **`config`** — TOML config at `~/.config/kondi/config.toml`. Supports three server transports: `http`, `sse`, `stdio`. Auth values support `env:VAR_NAME` syntax to pull from environment variables.
- **`client`** — `ClientPool` connects to all configured upstream servers at startup using the `rmcp` crate. On tool call failure it automatically attempts one reconnect.
- **`catalog`** — Aggregates all tools from all connected servers into a flat list of `CatalogEntry` structs. Also generates TypeScript type declarations for each server's tools (used by the sandbox).
- **`sandbox`** — A QuickJS (`rquickjs`) JavaScript runtime that executes agent-written TypeScript. TypeScript is transpiled to JS via `oxc` before execution. The sandbox has a 64MB memory limit.
  - `search_code`: exposes a `tools` global (the full catalog as JSON) for filtering
  - `execute`: exposes each server as a typed JS Proxy object that dispatches calls to `ClientPool::call_tool`
- **`bm25`** — In-memory BM25 full-text index over the tool catalog, used by the `search` tool.
- **`transpile`** — Thin wrapper around `oxc` that strips TypeScript types and transpiles TS→JS.
- **`import`** — Reads `~/.claude.json` (Claude Desktop config) and converts its `mcpServers` to `ServerConfig` entries.
- **`admin`** — Shared request/response types for the admin API protocol.

### Admin API flow

The CLI communicates with the daemon via a JSON-over-HTTP admin API (`POST /admin`). When the daemon isn't running, the CLI auto-spawns it and waits up to 3 seconds. The admin API can reload the `ClientPool`, `Catalog`, `BM25Index`, and `Sandbox` atomically when servers are added/removed.

### MCP tools exposed by kondid

| Tool | Description |
|------|-------------|
| `search` | BM25 keyword search over all tools in the catalog |
| `search_code` | Run TypeScript against the `tools` array to filter/explore |
| `execute` | Run TypeScript that calls tools across any connected server |

## GUI development

```bash
make gui-setup    # install frontend dependencies (first time)
make gui-dev      # tauri dev server with hot-reload
make gui-build    # production bundle
```

The GUI lives in `crates/gui/` — a Tauri v2 app that is **not** part of the Cargo workspace (Tauri manages its own build). It communicates with `kondid` via the same HTTP admin API.

## Daemon lifecycle

```bash
kondi daemon install    # register kondid as a login item
kondi daemon start      # start daemon now
kondi daemon status     # check if running
kondi daemon stop       # stop daemon
kondi daemon restart    # stop then start
kondi daemon uninstall  # remove login item
```

Legacy aliases: `kondi mcp` (= `daemon start`) and `kondi stop` (= `daemon stop`) are kept for backwards compatibility.