# Phase 4a — `helix-claude-mcp` Foundation + MCP Resources — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the external `helix-claude-mcp` binary as a stdio MCP server that Claude Code can configure via `.mcp.json`. This phase ships the binary scaffolding, the JSON-RPC client that connects to Helix's control socket, the discovery logic that finds the right socket from `$CLAUDE_PROJECT_DIR`, and three MCP Resources backed by the snapshot file. Phase 4b adds Tools.

**Architecture:** A new workspace member `helix-claude-mcp` (depends on `helix-context-schema` for type sharing, plus `rmcp` for the MCP protocol). The binary has subcommands: `serve` (stdio MCP server) and `hook` (UserPromptSubmit handler, deferred to Phase 5). The `serve` subcommand starts an rmcp stdio server that exposes three Resources reading from `<workspace>/.helix/context.json`. The JSON-RPC client and discovery logic compile but are unused in Phase 4a — Resources don't need a socket connection. Phase 4b will use them for Tools.

**Tech Stack:** Rust 2021, `rmcp` (official Anthropic MCP SDK, crate name `rmcp` from `modelcontextprotocol/rust-sdk`), `tokio` (async runtime), `serde` + `serde_json`, `clap` (subcommand dispatch), `anyhow` (errors), `helix-context-schema` (shared schema types).

**Spec:** `docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md` §7 (helix-claude-mcp binary), §7.3 (MCP Resources vs Tools), §7.4 (discovery flow), §8 (Claude Code integration).

---

## Project context for the implementer

You are working in a Helix editor fork at `/Users/angm/helix` on branch `nightly`. Phases 1-3 are complete (tip: `a41e18c34`, 54 commits ahead of remote). The Helix-side control socket is fully functional: it binds at `<workspace>/.helix/control-<pid>.sock`, accepts JSON-RPC requests, and supports 11 methods (`initialize`, `current-state`, `get-open-buffers`, `get-buffer-text`, `open-file`, `goto-line`, `get-diagnostics`, `get-hover-at`, `get-definition-at`, `get-references-at`, `get-workspace-symbols`).

What Phase 4a adds:

- A new workspace member crate `helix-claude-mcp/` with `Cargo.toml` and binary `src/main.rs`.
- A JSON-RPC client module that connects to a Unix socket, sends a newline-delimited JSON request, and reads the newline-delimited JSON response.
- A discovery module that globs `$CLAUDE_PROJECT_DIR/.helix/control-*.sock` (plus `.sock.path` pointer files for macOS-long-path fallback) and picks a live socket.
- An rmcp-based stdio MCP server (`serve` subcommand) that responds to `initialize`, `resources/list`, and `resources/read`.
- Three MCP Resources backed by the snapshot file: `helix://state/current`, `helix://state/buffers`, `helix://state/snapshot`.

What Phase 4a does NOT do:

- MCP Tools — Phase 4b.
- The `hook` subcommand — Phase 5.
- `format-document` / `run-typable-command` — Phase 6.
- Wiring `.mcp.json` into your actual Claude Code config — left to you (it's a per-project decision, not something this plan should change for you automatically). The smoke test uses a temporary `.mcp.json` in a throwaway directory.

## File structure

**Create:**

- `helix-claude-mcp/Cargo.toml` — manifest.
- `helix-claude-mcp/src/main.rs` — entry point, clap subcommand dispatch.
- `helix-claude-mcp/src/serve.rs` — stdio MCP server (`serve` subcommand body).
- `helix-claude-mcp/src/discovery.rs` — find a live Helix socket from `$CLAUDE_PROJECT_DIR`.
- `helix-claude-mcp/src/rpc_client.rs` — Unix-socket JSON-RPC client (sync request-response over async I/O).
- `helix-claude-mcp/src/resources.rs` — three MCP Resource handlers.
- `helix-claude-mcp/tests/integration.rs` — integration test of the stdio MCP server (spawns the binary, sends MCP messages on its stdin, reads stdout).

**Modify:**

- `Cargo.toml` (workspace root) — add `helix-claude-mcp` to `members`.

**No changes to existing crates** — this phase only adds a new crate. The JSON-RPC client module that Phase 4b will use to drive Helix is built here but used only minimally in Phase 4a.

---

## Task 1: Create `helix-claude-mcp` crate skeleton

**Files:**
- Create: `helix-claude-mcp/Cargo.toml`
- Create: `helix-claude-mcp/src/main.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create the crate manifest**

Create `helix-claude-mcp/Cargo.toml`:

```toml
[package]
name = "helix-claude-mcp"
description = "MCP bridge that exposes Helix editor state and commands to Claude Code."
include = ["src/**/*", "README.md"]
version.workspace = true
authors.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
categories.workspace = true
repository.workspace = true
homepage.workspace = true

[[bin]]
name = "helix-claude-mcp"
path = "src/main.rs"

[dependencies]
helix-context-schema = { path = "../helix-context-schema" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net", "io-util", "time"] }
anyhow = "1.0"
clap = { version = "4", features = ["derive"] }
log = "0.4"
env_logger = "0.11"
rmcp = { version = "0.1", features = ["server", "transport-io"] }

[dev-dependencies]
tempfile = { workspace = true }
```

**Note on `rmcp`:** the version `0.1` is a placeholder. Check crates.io for the current version (`cargo search rmcp` or `cargo info rmcp`). Use the latest stable. Also confirm the feature flags (`server`, `transport-io`) exist — they may be named differently in current rmcp. Adjust based on what `cargo check` and rmcp's README tell you.

- [ ] **Step 2: Add the crate to the workspace `members` list**

Edit `/Users/angm/helix/Cargo.toml`. Find `[workspace] members = [...]`. Add `"helix-claude-mcp"` alphabetically (between `helix-context-schema` and `helix-core`):

```toml
members = [
  "helix-context-schema",
  "helix-claude-mcp",
  "helix-core",
  ...
]
```

Do **not** add it to `default-members` — we don't want `cargo build` (without args) to build the bridge by default.

- [ ] **Step 3: Create `src/main.rs` skeleton**

Create `helix-claude-mcp/src/main.rs`:

```rust
//! `helix-claude-mcp` — MCP bridge that exposes the Helix editor's control
//! socket through the Model Context Protocol to Claude Code.
//!
//! Subcommands:
//! - `serve`: stdio MCP server. Configured in Claude Code via `.mcp.json`.
//! - `hook`: UserPromptSubmit hook handler (Phase 5).

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "helix-claude-mcp", version)]
#[command(about = "MCP bridge for the Helix editor's control socket")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the stdio MCP server. Configured in Claude Code's .mcp.json.
    Serve,
    /// Run the UserPromptSubmit hook (Phase 5; not yet implemented).
    Hook,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .target(env_logger::Target::Stderr) // stdout is reserved for MCP framing
    .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve => {
            log::info!("helix-claude-mcp serve starting");
            // Phase 4a Task 4 wires up the rmcp server here.
            anyhow::bail!("serve not yet implemented");
        }
        Command::Hook => {
            anyhow::bail!("hook is a Phase 5 deliverable");
        }
    }
}
```

- [ ] **Step 4: Verify build**

Run: `cargo check -p helix-claude-mcp`
Expected: Successful build of the crate (it has no real code yet, just the clap skeleton).

Run: `cargo check --workspace`
Expected: Clean.

**If `rmcp` doesn't compile or doesn't exist on crates.io at the version you picked:** the implementer should `cargo search rmcp` to find the current name and version. The official Anthropic Rust SDK was published as `rmcp` at one point but the name and shape may have evolved. Adjust the dep and the feature flags as needed. If `rmcp` is unavailable, document that in the commit message and remove the dep for now — Task 4 will reintroduce it once we know the correct crate name.

- [ ] **Step 5: Verify the binary boots**

Run: `cargo run -p helix-claude-mcp -- --help`
Expected: clap help output showing `serve` and `hook` subcommands.

Run: `cargo run -p helix-claude-mcp -- serve`
Expected: Error message "serve not yet implemented" (this is correct for this task).

- [ ] **Step 6: Commit**

```bash
git add helix-claude-mcp/ Cargo.toml
git commit -m "$(cat <<'EOF'
feat(claude-mcp): crate skeleton with clap subcommands

New workspace member helix-claude-mcp/ — the future MCP bridge between
Claude Code and Helix's control socket. Binary with two subcommands:
- serve: stdio MCP server (Phase 4)
- hook: UserPromptSubmit handler (Phase 5)

Both subcommands currently bail with "not yet implemented" — Tasks 2-7
of Phase 4a wire up `serve`.

env_logger logs to stderr (stdout is reserved for MCP framing).

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7)
EOF
)"
```

---

## Task 2: JSON-RPC client over Unix socket

**Files:**
- Create: `helix-claude-mcp/src/rpc_client.rs`
- Modify: `helix-claude-mcp/src/main.rs`

Phase 4b's Tools need this. Phase 4a doesn't strictly need it (Resources read from the snapshot file), but building it now keeps the foundation complete.

- [ ] **Step 1: Write failing tests**

Create `helix-claude-mcp/src/rpc_client.rs`:

```rust
//! JSON-RPC client that connects to Helix's control socket and exchanges
//! newline-delimited JSON-RPC messages. One request, one response per call;
//! does not reuse connections (Phase 4b may add connection pooling if
//! needed).

use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use helix_context_schema::{ControlRequest, ControlResponse, JsonRpcError};

const RECV_BUF_INITIAL: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("connect to {0}: {1}")]
    Connect(std::path::PathBuf, std::io::Error),
    #[error("write request: {0}")]
    Write(std::io::Error),
    #[error("read response: {0}")]
    Read(std::io::Error),
    #[error("peer closed connection before sending a response")]
    PeerClosed,
    #[error("response was not valid JSON-RPC: {0}")]
    Parse(serde_json::Error),
    #[error("helix returned an error: {message} (code {code})", code = .0.code as i32, message = .0.message)]
    HelixError(JsonRpcError),
}

/// Connect to `socket_path`, send `request`, read one line of response,
/// parse it as either a successful `ControlResponse` or a `JsonRpcError`.
pub async fn send_request(
    socket_path: &Path,
    request: &ControlRequest,
) -> Result<ControlResponse, RpcError> {
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| RpcError::Connect(socket_path.to_path_buf(), e))?;
    let (read_half, mut write_half) = stream.into_split();

    let mut payload = serde_json::to_vec(request)
        .map_err(|e| RpcError::Parse(e))?;
    payload.push(b'\n');
    write_half.write_all(&payload).await.map_err(RpcError::Write)?;
    write_half.flush().await.map_err(RpcError::Write)?;

    let mut reader = BufReader::with_capacity(RECV_BUF_INITIAL, read_half);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await.map_err(RpcError::Read)?;
    if n == 0 {
        return Err(RpcError::PeerClosed);
    }

    // The line is either {"method":"...","result":{...}} or {"code":...,"message":...,...}
    // Try response first; if that fails, try error.
    if let Ok(resp) = serde_json::from_str::<ControlResponse>(line.trim()) {
        return Ok(resp);
    }
    let err: JsonRpcError = serde_json::from_str(line.trim())
        .map_err(RpcError::Parse)?;
    Err(RpcError::HelixError(err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_context_schema::{ClientInfo, ControlResponse, PROTOCOL_VERSION};
    use std::io::Write;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    /// Spawn a fake Helix-side that accepts one connection, reads one line,
    /// responds with a canned response, then exits.
    async fn spawn_fake_helix(
        socket_path: std::path::PathBuf,
        response_line: String,
    ) -> tokio::task::JoinHandle<()> {
        let listener = UnixListener::bind(&socket_path).unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            // read just enough — we don't validate; we send the canned response.
            use tokio::io::AsyncReadExt;
            let _ = stream.read(&mut buf).await.unwrap();
            use tokio::io::AsyncWriteExt;
            stream.write_all(response_line.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();
        })
    }

    #[tokio::test]
    async fn send_request_parses_success_response() {
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("fake.sock");
        let canned = r#"{"method":"initialize","result":{"protocol_version":"1.0","helix_version":"25.7.1","server_info":{"name":"helix","version":"25.7.1"},"capabilities":{"read_methods":[],"write_methods":[]}}}"#;
        let _server = spawn_fake_helix(sock.clone(), format!("{}\n", canned)).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let req = ControlRequest::Initialize {
            protocol_version: PROTOCOL_VERSION.into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let resp = send_request(&sock, &req).await.unwrap();
        let ControlResponse::Initialize { protocol_version, .. } = resp else {
            panic!("wrong variant");
        };
        assert_eq!(protocol_version, "1.0");
    }

    #[tokio::test]
    async fn send_request_parses_error_response() {
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("fake-err.sock");
        let canned = r#"{"code":-32601,"message":"Method 'foo' not found"}"#;
        let _server = spawn_fake_helix(sock.clone(), format!("{}\n", canned)).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let req = ControlRequest::Initialize {
            protocol_version: PROTOCOL_VERSION.into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let err = send_request(&sock, &req).await.unwrap_err();
        match err {
            RpcError::HelixError(je) => assert_eq!(je.message, "Method 'foo' not found"),
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[tokio::test]
    async fn send_request_returns_connect_error_for_missing_socket() {
        let nonexistent = std::path::PathBuf::from("/tmp/definitely-not-here.sock");
        let req = ControlRequest::Initialize {
            protocol_version: PROTOCOL_VERSION.into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let err = send_request(&nonexistent, &req).await.unwrap_err();
        assert!(matches!(err, RpcError::Connect(_, _)), "got: {:?}", err);
    }
}
```

- [ ] **Step 2: Add `thiserror` to Cargo.toml**

Edit `helix-claude-mcp/Cargo.toml`. Under `[dependencies]`, add:

```toml
thiserror = { workspace = true }
```

(`thiserror` is already in `[workspace.dependencies]` per the existing workspace setup; if not, use direct version `"2.0"`.)

- [ ] **Step 3: Wire the module into main.rs**

Edit `helix-claude-mcp/src/main.rs`. Near the top (after `use clap::...`), add:

```rust
mod rpc_client;
```

- [ ] **Step 4: Verify build and run tests**

Run: `cargo test -p helix-claude-mcp`
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add helix-claude-mcp/src/rpc_client.rs helix-claude-mcp/src/main.rs helix-claude-mcp/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(claude-mcp): JSON-RPC client over Unix socket

send_request connects, writes one newline-delimited JSON request, reads
one newline-delimited response. Parses the response either as
ControlResponse on success or JsonRpcError on failure (the wire format
shape determines which).

Three tests with a fake Helix-side listener: success response,
JsonRpcError response, connect failure. No actual Helix process needed
for tests.

The client is connection-per-request — no pooling. Phase 4b can add
that if MCP request volume warrants it; for the expected workload (1-2
calls per Claude prompt) the simplicity wins.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.5)
EOF
)"
```

---

## Task 3: Discovery module

**Files:**
- Create: `helix-claude-mcp/src/discovery.rs`
- Modify: `helix-claude-mcp/src/main.rs`

- [ ] **Step 1: Write failing tests**

Create `helix-claude-mcp/src/discovery.rs`:

```rust
//! Find the right Helix control socket for the current workspace.
//!
//! Per spec §7.4, discovery globs `<workspace>/.helix/control-*.sock` plus
//! any pointer files (`*.sock.path` — used when the project-local path
//! would exceed sun_path). Filters out unconnectable sockets via a brief
//! connect attempt. If multiple live sockets exist, picks the one with the
//! newest mtime.

use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::net::UnixStream;

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("CLAUDE_PROJECT_DIR not set and no working directory available")]
    NoWorkspace,
    #[error("no live Helix control socket found in {0}/.helix/")]
    NoLiveSocket(PathBuf),
    #[error("reading .helix dir: {0}")]
    Io(#[from] std::io::Error),
}

/// Discover the live Helix control socket. Returns the path that should be
/// passed to `rpc_client::send_request`.
///
/// `workspace_override` lets callers (and tests) skip env-var lookup.
pub async fn find_helix_socket(
    workspace_override: Option<&Path>,
) -> Result<PathBuf, DiscoveryError> {
    let workspace = match workspace_override {
        Some(p) => p.to_path_buf(),
        None => {
            std::env::var_os("CLAUDE_PROJECT_DIR")
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
                .ok_or(DiscoveryError::NoWorkspace)?
        }
    };

    let helix_dir = workspace.join(".helix");
    if !helix_dir.exists() {
        return Err(DiscoveryError::NoLiveSocket(workspace));
    }

    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    let mut dir = tokio::fs::read_dir(&helix_dir).await?;
    while let Some(entry) = dir.next_entry().await? {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let socket_path = if name.starts_with("control-") && name.ends_with(".sock") {
            path.clone()
        } else if name.starts_with("control-") && name.ends_with(".sock.path") {
            // Read the pointer file to find the real socket location.
            match tokio::fs::read_to_string(&path).await {
                Ok(contents) => PathBuf::from(contents.trim()),
                Err(_) => continue,
            }
        } else {
            continue;
        };
        let mtime = entry
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if is_socket_live(&socket_path).await {
            candidates.push((socket_path, mtime));
        }
    }

    candidates.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
    candidates
        .into_iter()
        .next()
        .map(|(p, _)| p)
        .ok_or(DiscoveryError::NoLiveSocket(workspace))
}

/// Try to connect to the socket with a 200 ms timeout. ECONNREFUSED or
/// ENOENT (stale entries) return false. A successful connect returns true
/// and the connection is immediately dropped.
async fn is_socket_live(path: &Path) -> bool {
    match tokio::time::timeout(Duration::from_millis(200), UnixStream::connect(path)).await {
        Ok(Ok(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn returns_no_live_socket_when_helix_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let err = find_helix_socket(Some(tmp.path())).await.unwrap_err();
        assert!(matches!(err, DiscoveryError::NoLiveSocket(_)));
    }

    #[tokio::test]
    async fn returns_no_live_socket_when_only_stale_files_exist() {
        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        // Create a file that LOOKS like a socket but isn't bound.
        std::fs::File::create(helix.join("control-99999.sock")).unwrap();
        let err = find_helix_socket(Some(tmp.path())).await.unwrap_err();
        assert!(matches!(err, DiscoveryError::NoLiveSocket(_)));
    }

    #[tokio::test]
    async fn returns_live_socket_path() {
        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        let sock = helix.join("control-12345.sock");
        let _listener = UnixListener::bind(&sock).unwrap();

        let resolved = find_helix_socket(Some(tmp.path())).await.unwrap();
        assert_eq!(resolved, sock);
    }

    #[tokio::test]
    async fn follows_pointer_file() {
        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        // Create the real socket somewhere outside the .helix dir.
        let real_sock = tmp.path().join("real.sock");
        let _listener = UnixListener::bind(&real_sock).unwrap();
        // Pointer file at expected location.
        let pointer = helix.join("control-12345.sock.path");
        std::fs::write(&pointer, real_sock.to_str().unwrap()).unwrap();

        let resolved = find_helix_socket(Some(tmp.path())).await.unwrap();
        assert_eq!(resolved, real_sock);
    }

    #[tokio::test]
    async fn picks_newest_when_multiple_live() {
        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        let older = helix.join("control-100.sock");
        let newer = helix.join("control-200.sock");
        let _l1 = UnixListener::bind(&older).unwrap();
        // Sleep so mtimes differ.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _l2 = UnixListener::bind(&newer).unwrap();

        let resolved = find_helix_socket(Some(tmp.path())).await.unwrap();
        assert_eq!(resolved, newer);
    }
}
```

- [ ] **Step 2: Wire into main.rs**

Edit `helix-claude-mcp/src/main.rs`. Add:

```rust
mod discovery;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p helix-claude-mcp`
Expected: 3 (rpc_client) + 5 (discovery) = 8 tests pass.

- [ ] **Step 4: Commit**

```bash
git add helix-claude-mcp/src/discovery.rs helix-claude-mcp/src/main.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): socket discovery from workspace

find_helix_socket globs <workspace>/.helix/control-*.sock plus pointer
files (*.sock.path) per spec §7.4. Filters via 200ms connect probe;
picks newest mtime when multiple live sockets exist.

Workspace resolution order: explicit override > CLAUDE_PROJECT_DIR env
> current working directory.

Five tests: missing .helix/ dir, stale-only files, single live socket,
pointer-file follow, newest-of-multiple selection.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.4)
EOF
)"
```

---

## Task 4: rmcp stdio server scaffolding

**Files:**
- Create: `helix-claude-mcp/src/serve.rs`
- Modify: `helix-claude-mcp/src/main.rs`

This is the friction-heaviest task because `rmcp`'s API is unknown until you actually use it. The plan provides a sketch; the implementer adjusts based on what `cargo doc -p rmcp --open` shows.

- [ ] **Step 1: Investigate `rmcp`'s server API**

Run: `cargo doc -p rmcp --no-deps`
Open `target/doc/rmcp/index.html` and look at:
- How to define a server (likely a struct implementing some `Server` or `Service` trait)
- How to register tools and resources
- How to run a stdio transport

The README at <https://github.com/modelcontextprotocol/rust-sdk> is the authoritative source for the current API. Don't trust this plan's specific function names — use the actual SDK.

If the SDK has examples in `examples/`, copy the simplest stdio-server example as a starting template.

- [ ] **Step 2: Create `serve.rs` with a minimal MCP server**

The sketch below is illustrative; adapt to the actual rmcp API. The principles are:
- Run an rmcp stdio server.
- Respond to `initialize` with server info from `CARGO_PKG_VERSION`.
- For Phase 4a, advertise three resources by URI (`helix://state/current`, `helix://state/buffers`, `helix://state/snapshot`). Real handlers come in Task 5.
- No tools yet — Phase 4b.

Skeleton:

```rust
//! `serve` subcommand: stdio MCP server.
//!
//! Reads MCP protocol on stdin, writes responses on stdout. stderr is
//! reserved for logs (env_logger). Run by Claude Code via the `.mcp.json`
//! config in a project workspace.

use anyhow::Result;

pub async fn run() -> Result<()> {
    log::info!("starting stdio MCP server");

    // Build the rmcp server. The exact API is rmcp-specific; the implementer
    // should consult `cargo doc -p rmcp` for the current shape. The target
    // architecture is:
    //
    // 1. Construct a server with name/version from env!("CARGO_PKG_VERSION").
    // 2. Register the three resources (URIs only at this stage — Task 5
    //    adds real handlers).
    // 3. Bind a stdio transport and run the server.

    // PLACEHOLDER — the implementer fills this in based on rmcp's API.
    // For Phase 4a Task 4, even an `anyhow::bail!("rmcp wiring TODO Task 5")`
    // is acceptable if the rmcp API turns out more involved than expected.

    anyhow::bail!("rmcp server wiring TODO — implementer to complete in Task 5")
}
```

- [ ] **Step 3: Wire into main.rs**

Edit `helix-claude-mcp/src/main.rs`. Add the module:

```rust
mod serve;
```

Change the `Command::Serve` branch:

```rust
        Command::Serve => {
            serve::run().await?;
            Ok(())
        }
```

- [ ] **Step 4: Verify build**

Run: `cargo check -p helix-claude-mcp`
Expected: Clean (serve::run panics at runtime but compiles).

Run: `cargo test -p helix-claude-mcp`
Expected: 8 tests still pass.

- [ ] **Step 5: Commit**

```bash
git add helix-claude-mcp/src/serve.rs helix-claude-mcp/src/main.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): serve subcommand skeleton

Adds the serve::run entry point. Currently bails — Task 5 fills in the
actual rmcp server setup once the SDK's API is verified against
`cargo doc -p rmcp`.

This commit keeps the wiring shape ready for the next task: main.rs
dispatches `Command::Serve` to `serve::run().await`, env_logger logs
to stderr so stdout stays clean for MCP framing.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7)
EOF
)"
```

---

## Task 5: MCP Resources backed by the snapshot file

**Files:**
- Create: `helix-claude-mcp/src/resources.rs`
- Modify: `helix-claude-mcp/src/serve.rs`
- Modify: `helix-claude-mcp/src/main.rs`

This task finishes the rmcp server wiring AND adds the three Resources. They're combined because the rmcp registration is intertwined.

- [ ] **Step 1: Inspect `rmcp` examples for resource handlers**

`cargo doc -p rmcp --no-deps` and the SDK's examples are authoritative. Look specifically for:
- How to register a Resource by URI.
- How to provide a handler for `resources/read` that returns `Vec<ResourceContents>` (or whatever rmcp calls it).

- [ ] **Step 2: Create resource handlers**

Create `helix-claude-mcp/src/resources.rs`:

```rust
//! MCP Resource handlers. All three read from `<workspace>/.helix/context.json`
//! — Helix's snapshot file. Tools (Phase 4b) will use the socket for live
//! data; Resources stay on the cheap file-read path.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use helix_context_schema::ContextSnapshot;

#[derive(Debug, Clone, Copy)]
pub enum ResourceKind {
    /// `helix://state/current` — the active buffer's state (path, cursor,
    /// selection, mode).
    Current,
    /// `helix://state/buffers` — the list of open buffers.
    Buffers,
    /// `helix://state/snapshot` — the entire snapshot file.
    Snapshot,
}

impl ResourceKind {
    pub const fn uri(self) -> &'static str {
        match self {
            Self::Current => "helix://state/current",
            Self::Buffers => "helix://state/buffers",
            Self::Snapshot => "helix://state/snapshot",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Current => "helix:state:current",
            Self::Buffers => "helix:state:buffers",
            Self::Snapshot => "helix:state:snapshot",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Current => {
                "The currently focused buffer's path, cursor, selection, language, and editor mode."
            }
            Self::Buffers => {
                "List of all open buffers in the running Helix instance."
            }
            Self::Snapshot => {
                "Full snapshot file: timestamp, project root, instance info, active buffer, open buffers."
            }
        }
    }

    pub fn all() -> impl Iterator<Item = Self> {
        [Self::Current, Self::Buffers, Self::Snapshot].into_iter()
    }
}

/// Read the snapshot file. None when missing (Helix not running or
/// context-logger disabled) — that's a normal state, callers handle it
/// by returning a friendly "no snapshot available" Resource body.
fn load_snapshot(workspace: &Path) -> Option<ContextSnapshot> {
    let path = workspace.join(".helix").join("context.json");
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Resolve `<workspace>` for the resource read. Mirrors discovery: env
/// override > CLAUDE_PROJECT_DIR > current dir.
pub fn resolve_workspace(workspace_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = workspace_override {
        return Ok(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from) {
        return Ok(p);
    }
    std::env::current_dir().context("no CLAUDE_PROJECT_DIR and current_dir unavailable")
}

/// Produce the resource body for the given URI. Returns a JSON string in
/// the appropriate shape for rmcp's resource read response. The MIME type
/// is `application/json` for all three.
pub fn read_resource(kind: ResourceKind, workspace: &Path) -> String {
    let snap = match load_snapshot(workspace) {
        Some(s) => s,
        None => {
            return serde_json::json!({
                "error": "no snapshot available",
                "hint": "Helix isn't running, or [editor.context-logger] enabled = false.",
            })
            .to_string();
        }
    };

    match kind {
        ResourceKind::Current => serde_json::json!({
            "active": snap.active,
            "mode": snap.mode,
            "project_root": snap.project_root,
            "timestamp": snap.timestamp,
            "last_update_source": snap.last_update_source,
        })
        .to_string(),
        ResourceKind::Buffers => serde_json::json!({
            "buffers": snap.open_buffers,
            "timestamp": snap.timestamp,
        })
        .to_string(),
        ResourceKind::Snapshot => serde_json::to_string(&snap)
            .unwrap_or_else(|_| "{}".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_snapshot(workspace: &Path, json: &str) {
        let helix = workspace.join(".helix");
        std::fs::create_dir_all(&helix).unwrap();
        std::fs::write(helix.join("context.json"), json).unwrap();
    }

    fn minimal_snapshot() -> String {
        serde_json::json!({
            "schema_version": 2,
            "min_supported_reader": 1,
            "timestamp": "2026-05-13T10:00:00Z",
            "last_update_source": "focus_lost",
            "project_root": "/tmp/test",
            "mode": "normal",
            "active": {
                "path": "main.rs",
                "path_abs": "/tmp/test/main.rs",
                "language": "rust",
                "modified": false,
                "line_count": 5,
                "cursors": [{"primary": true, "line": 1, "column": 1}],
                "selections": []
            },
            "open_buffers": [
                {"path": "/tmp/test/main.rs", "language": "rust", "modified": false}
            ]
        })
        .to_string()
    }

    #[test]
    fn current_resource_returns_active_block() {
        let tmp = TempDir::new().unwrap();
        write_snapshot(tmp.path(), &minimal_snapshot());
        let body = read_resource(ResourceKind::Current, tmp.path());
        let j: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(j["active"]["path"], "main.rs");
        assert_eq!(j["mode"], "normal");
    }

    #[test]
    fn buffers_resource_returns_open_buffers_list() {
        let tmp = TempDir::new().unwrap();
        write_snapshot(tmp.path(), &minimal_snapshot());
        let body = read_resource(ResourceKind::Buffers, tmp.path());
        let j: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(j["buffers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn snapshot_resource_returns_full_snapshot() {
        let tmp = TempDir::new().unwrap();
        write_snapshot(tmp.path(), &minimal_snapshot());
        let body = read_resource(ResourceKind::Snapshot, tmp.path());
        let j: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(j["schema_version"], 2);
        assert_eq!(j["project_root"], "/tmp/test");
    }

    #[test]
    fn missing_snapshot_returns_friendly_error_body() {
        let tmp = TempDir::new().unwrap();
        // No .helix dir, no snapshot
        let body = read_resource(ResourceKind::Current, tmp.path());
        let j: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(j["error"].is_string());
    }

    #[test]
    fn all_kinds_iterates_three_kinds() {
        let kinds: Vec<_> = ResourceKind::all().collect();
        assert_eq!(kinds.len(), 3);
        let uris: Vec<_> = kinds.iter().map(|k| k.uri()).collect();
        assert!(uris.contains(&"helix://state/current"));
        assert!(uris.contains(&"helix://state/buffers"));
        assert!(uris.contains(&"helix://state/snapshot"));
    }
}
```

- [ ] **Step 3: Wire into serve.rs (the rmcp server)**

Replace the placeholder in `helix-claude-mcp/src/serve.rs` with the rmcp setup that registers `ResourceKind::all()` and dispatches reads to `resources::read_resource`. This is where the implementer's investigation of rmcp's API pays off.

Conceptual shape (adapt to rmcp's actual types):

```rust
use anyhow::Result;
use crate::resources::{ResourceKind, read_resource, resolve_workspace};

pub async fn run() -> Result<()> {
    log::info!("helix-claude-mcp serve starting");

    // Build the server using rmcp's high-level builder/service API.
    // The shape is:
    //
    //   let server = ServerBuilder::new(server_info)
    //       .with_resources(
    //           ResourceKind::all().map(|k| Resource {
    //               uri: k.uri().into(),
    //               name: k.name().into(),
    //               description: Some(k.description().into()),
    //               mime_type: Some("application/json".into()),
    //           })
    //       )
    //       .on_resource_read(|uri| async move {
    //           let kind = match uri.as_str() {
    //               u if u == ResourceKind::Current.uri() => ResourceKind::Current,
    //               u if u == ResourceKind::Buffers.uri() => ResourceKind::Buffers,
    //               u if u == ResourceKind::Snapshot.uri() => ResourceKind::Snapshot,
    //               _ => return Err(...),
    //           };
    //           let workspace = resolve_workspace(None)?;
    //           Ok(ResourceContents::text(uri, read_resource(kind, &workspace)))
    //       })
    //       .build()?;
    //
    //   server.serve_stdio().await?;

    // The implementer fills this in based on rmcp's actual API shape.
    // The key points are:
    //   - server_info uses CARGO_PKG_NAME and CARGO_PKG_VERSION
    //   - three resources registered with the URIs from ResourceKind::all()
    //   - resource read handler dispatches by URI
    //   - stdio transport
    //   - mime_type "application/json" on all three

    todo!("wire rmcp server — see comment above for shape")
}
```

If the actual rmcp API is dramatically different (e.g. async-trait Service rather than builder), restructure as needed. **The principle is: register the three resources, dispatch reads to `read_resource`, run on stdio.**

- [ ] **Step 4: Verify build**

Run: `cargo check -p helix-claude-mcp`
Expected: Clean once `todo!` is replaced with real rmcp code.

Run: `cargo test -p helix-claude-mcp`
Expected: 13 tests pass (3 rpc_client + 5 discovery + 5 resources).

- [ ] **Step 5: Manual smoke test**

```bash
mkdir -p /tmp/p4a-resources/.helix
cat > /tmp/p4a-resources/.helix/context.json <<'EOF'
{
  "schema_version": 2,
  "min_supported_reader": 1,
  "timestamp": "2026-05-13T10:00:00Z",
  "last_update_source": "focus_lost",
  "project_root": "/tmp/p4a-resources",
  "mode": "normal",
  "active": {
    "path": "main.rs",
    "path_abs": "/tmp/p4a-resources/main.rs",
    "language": "rust",
    "modified": false,
    "line_count": 5,
    "cursors": [{"primary": true, "line": 1, "column": 1}],
    "selections": []
  },
  "open_buffers": [
    {"path": "/tmp/p4a-resources/main.rs", "language": "rust", "modified": false}
  ]
}
EOF

# Build the binary
cargo build --release -p helix-claude-mcp

# Send MCP initialize + resources/list + resources/read on stdin
CLAUDE_PROJECT_DIR=/tmp/p4a-resources /Users/angm/helix/target/release/helix-claude-mcp serve <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"helix://state/current"}}
EOF
```

Expected: three JSON-RPC responses on stdout. The `resources/list` response contains the three URIs; the `resources/read` for `helix://state/current` contains the snapshot's active/mode/etc.

The exact MCP message shapes (newline-delimited or Content-Length-framed? `jsonrpc: "2.0"` envelope?) depend on what rmcp's stdio transport expects. Adapt the test input to whatever rmcp produces/consumes.

If running interactively fails, do the manual test differently: write an integration test (next task) that exercises the same end-to-end flow programmatically.

- [ ] **Step 6: Commit**

```bash
git add helix-claude-mcp/src/resources.rs helix-claude-mcp/src/serve.rs helix-claude-mcp/src/main.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): three MCP resources backed by the snapshot file

Resources read from <workspace>/.helix/context.json (the Helix
context-logger snapshot). Per spec §7.3, Resources stay on the cheap
file-read path; Tools (Phase 4b) will use the socket for live data.

ResourceKind::{Current, Buffers, Snapshot} centralizes URI / name /
description in one place — adding a fourth resource is a one-variant
change.

The rmcp server registers all three at startup, dispatches resources/read
by URI matching, and runs on stdio. MIME type application/json across
the board.

Five resource tests + manual smoke test confirm the end-to-end path
works against a sample snapshot file.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.3)
EOF
)"
```

---

## Task 6: Integration test — spawn binary, exchange MCP messages

**Files:**
- Create: `helix-claude-mcp/tests/integration.rs`

The smoke test from Task 5 is interactive. This task automates it.

- [ ] **Step 1: Write the integration test**

Create `helix-claude-mcp/tests/integration.rs`:

```rust
//! Integration test: spawn `helix-claude-mcp serve`, drive it via stdin/stdout
//! as Claude Code would.

use std::io::Write;
use std::process::Stdio;

use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const SAMPLE_SNAPSHOT: &str = r#"{
  "schema_version": 2,
  "min_supported_reader": 1,
  "timestamp": "2026-05-13T10:00:00Z",
  "last_update_source": "focus_lost",
  "project_root": "/tmp/p4a-test",
  "mode": "normal",
  "active": {
    "path": "main.rs",
    "path_abs": "/tmp/p4a-test/main.rs",
    "language": "rust",
    "modified": false,
    "line_count": 5,
    "cursors": [{"primary": true, "line": 1, "column": 1}],
    "selections": []
  },
  "open_buffers": [
    {"path": "/tmp/p4a-test/main.rs", "language": "rust", "modified": false}
  ]
}"#;

fn binary_path() -> std::path::PathBuf {
    // CARGO_BIN_EXE_helix-claude-mcp is set by cargo for integration tests.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_helix-claude-mcp"))
}

#[tokio::test]
async fn initialize_handshake_succeeds() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    let mut child = Command::new(binary_path())
        .arg("serve")
        .env("CLAUDE_PROJECT_DIR", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helix-claude-mcp");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let init_req =
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#;
    stdin.write_all(init_req.as_bytes()).await.unwrap();
    stdin.write_all(b"\n").await.unwrap();
    stdin.flush().await.unwrap();

    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    assert!(line.contains("\"result\""), "initialize response: {}", line);
    assert!(
        line.contains("\"serverInfo\"") || line.contains("\"server_info\""),
        "missing serverInfo: {}", line,
    );

    // Tear down
    drop(stdin);
    let _ = child.kill().await;
}

#[tokio::test]
async fn resources_list_returns_three_uris() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    let mut child = Command::new(binary_path())
        .arg("serve")
        .env("CLAUDE_PROJECT_DIR", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Send initialize, initialized notification, then resources/list
    for msg in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    // Read until we see the resources/list response (id: 2)
    let mut found = false;
    for _ in 0..5 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                assert!(line.contains("helix://state/current"), "missing current: {}", line);
                assert!(line.contains("helix://state/buffers"), "missing buffers: {}", line);
                assert!(line.contains("helix://state/snapshot"), "missing snapshot: {}", line);
                found = true;
                break;
            }
        }
    }
    assert!(found, "didn't see resources/list response");

    drop(stdin);
    let _ = child.kill().await;
}

#[tokio::test]
async fn resources_read_current_returns_active_block() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    let mut child = Command::new(binary_path())
        .arg("serve")
        .env("CLAUDE_PROJECT_DIR", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    for msg in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"helix://state/current"}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..5 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                // The result wraps the resource contents. Expect the JSON to
                // contain `main.rs` and `normal` (from active.path and mode).
                assert!(line.contains("main.rs"), "missing path: {}", line);
                assert!(line.contains("normal"), "missing mode: {}", line);
                found = true;
                break;
            }
        }
    }
    assert!(found, "didn't see resources/read response");

    drop(stdin);
    let _ = child.kill().await;
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test -p helix-claude-mcp --test integration`
Expected: 3 tests pass.

If any tests fail because rmcp's wire shape differs (e.g. uses Content-Length framing, or wraps responses differently than the asserts expect), adjust the asserts accordingly. The point is: spawning the binary and exchanging MCP messages via stdin/stdout works end-to-end.

- [ ] **Step 3: Run all tests**

Run: `cargo test -p helix-claude-mcp`
Expected: 13 unit tests + 3 integration tests = 16 pass.

- [ ] **Step 4: Commit**

```bash
git add helix-claude-mcp/tests/integration.rs
git commit -m "$(cat <<'EOF'
test(claude-mcp): integration tests for the stdio MCP server

Three integration tests that spawn the binary as a subprocess, drive it
via stdin/stdout exactly as Claude Code would:

1. initialize handshake — verify response contains serverInfo
2. resources/list — verify all three URIs are advertised
3. resources/read helix://state/current — verify the active block from
   the snapshot makes it back to the caller

CLAUDE_PROJECT_DIR is set per-test to a temp dir containing a sample
snapshot. No real Helix process needed.

Tests rely on CARGO_BIN_EXE_helix-claude-mcp, set automatically by
cargo for integration tests against the binary.
EOF
)"
```

---

## Task 7: Documentation + `.mcp.json` example

**Files:**
- Create: `helix-claude-mcp/README.md`
- Optionally: provide an example `.mcp.json` snippet (in README; do not modify any user-facing `.mcp.json` in the repo)

- [ ] **Step 1: Write `helix-claude-mcp/README.md`**

Create `helix-claude-mcp/README.md`:

```markdown
# helix-claude-mcp

MCP bridge that exposes Helix editor state and commands to Claude Code.

## What this is

A small Rust binary that bridges two pieces:

- **Helix's control socket** at `<workspace>/.helix/control-<pid>.sock` (a custom JSON-RPC dialect spoken by Helix's `[editor.control-socket]` feature). See `../docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md`.
- **Claude Code's MCP** (Model Context Protocol) over stdio.

Claude Code's `.mcp.json` configures this binary as a stdio MCP server. Once it's running, Claude can:

- Read editor state via MCP **Resources** (`helix://state/current`, `helix://state/buffers`, `helix://state/snapshot`).
- Drive the editor via MCP **Tools** (open files, jump to lines, query LSP). *(Phase 4b — not yet implemented.)*

## Installation

From the workspace root:

```bash
cargo build --release -p helix-claude-mcp
cp target/release/helix-claude-mcp ~/.cargo/bin/
```

Or run it directly from `target/release/helix-claude-mcp`.

## Claude Code configuration

Add this to your project's `.mcp.json` (or to your global `~/.claude.json` MCP servers list):

```json
{
  "mcpServers": {
    "helix": {
      "command": "helix-claude-mcp",
      "args": ["serve"]
    }
  }
}
```

Claude Code spawns the process per session and sets `CLAUDE_PROJECT_DIR` automatically.

## Helix configuration

In `~/.config/helix/config.toml`:

```toml
[editor.context-logger]
enabled = true

[editor.control-socket]
enabled = true
```

## How it works

- **Resources** read from the snapshot file `<workspace>/.helix/context.json` — fast, no Helix process required (returns a friendly error if the snapshot is missing).
- **Tools** (Phase 4b) connect to the live Helix control socket via discovery — globbing `<workspace>/.helix/control-*.sock` and picking the live one.

## Subcommands

- `helix-claude-mcp serve` — stdio MCP server.
- `helix-claude-mcp hook` — UserPromptSubmit hook handler. *(Phase 5 — not yet implemented.)*
```

- [ ] **Step 2: Verify the README**

Open it (mentally or via `cat`) and confirm it reads cleanly and matches the actual binary behavior.

- [ ] **Step 3: Commit**

```bash
git add helix-claude-mcp/README.md
git commit -m "$(cat <<'EOF'
docs(claude-mcp): README explaining install and Claude Code wiring

Top-level README for the new crate. Covers what it is, how to install,
the .mcp.json snippet, the Helix config side, and the high-level
mechanics (Resources read snapshot file, Tools will use socket).
EOF
)"
```

---

## Self-review checklist (for the implementer)

After all 7 tasks:

- [ ] `cargo check --workspace` clean
- [ ] `cargo test -p helix-claude-mcp` — 16 tests pass (3 rpc_client + 5 discovery + 5 resources + 3 integration)
- [ ] `cargo test -p helix-context-schema` still 44 pass (no schema changes in Phase 4a)
- [ ] `cargo test -p helix-term control_socket` still 15 pass (no helix-term changes)
- [ ] `cargo build --release -p helix-claude-mcp` produces a working binary
- [ ] Manual smoke test from Task 5 reproduces — `resources/list` returns three URIs, `resources/read` returns active block
- [ ] `git log --oneline -8` shows the 7 Phase 4a commits cleanly

## What's NOT in Phase 4a

- MCP Tools — Phase 4b.
- The `hook` subcommand — Phase 5.
- `format-document`, `run-typable-command` — Phase 6.
- Wiring `.mcp.json` into your active Claude Code config — left to you (per-project decision).

## Open questions for the implementer

1. **`rmcp` API.** The plan provides illustrative code shapes but the actual SDK API may differ in non-trivial ways (builder vs. trait, sync vs. async handlers, response wrapping). Use `cargo doc -p rmcp --no-deps --open` and the SDK's examples as the source of truth. If something doesn't fit the plan's shape, restructure — the principle (register three resources, dispatch by URI, run on stdio) is what matters.

2. **Wire format.** MCP's stdio transport is newline-delimited JSON with the `jsonrpc: "2.0"` envelope. Confirm rmcp produces/consumes exactly this. If it uses Content-Length framing or something else, the integration tests need to be adjusted.

3. **`thiserror` availability in workspace.** The plan assumes `thiserror` is in `[workspace.dependencies]`. If it isn't, add it directly with a recent version (`thiserror = "2.0"`).

4. **The placeholder `serve` body in Task 4.** Task 4 lands a server stub that bails. Task 5 replaces the stub with real rmcp code. The intermediate state is intentional — it lets Task 4 commit independently with cargo check clean. If you'd rather collapse Tasks 4 + 5 into a single commit, that's fine — just adjust the commit message accordingly.
