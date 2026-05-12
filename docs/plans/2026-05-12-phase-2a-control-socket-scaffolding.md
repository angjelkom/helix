# Phase 2a — Control Socket Scaffolding + `initialize` Handshake — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Unix-socket JSON-RPC server inside Helix at `<workspace>/.helix/control-<pid>.sock`, listening for control requests from external tools (the future `helix-claude-mcp` bridge). Implement the `initialize` handshake end-to-end as a complete vertical slice. No editor mutation or LSP queries yet — those land in Phase 2b/2c.

**Architecture:** New `control_socket` module in `helix-term` owns the listener, per-connection tasks, and JSON-RPC framing. Control messages travel as a new `EditorEvent::ControlRequest` variant carrying a `oneshot::Sender` reply channel — this lets per-connection tokio tasks forward requests into the main event loop where `&mut Editor` is available, and receive responses asynchronously. Shared protocol types live in `helix-context-schema` so the future MCP bridge can depend on the same definitions.

**Tech Stack:** Tokio (already in workspace), `tokio::net::UnixListener`, `tokio::sync::{mpsc, oneshot}`, serde + serde_json (already deps of `helix-context-schema`), `libc` for umask (already used elsewhere in `helix-term`).

**Spec:** `docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md`, Phase 2 (§11) — specifically the socket lifecycle (§5.2, §5.3), event-loop integration (§5.4), JSON-RPC dialect (§6.1, §6.4), and the `initialize` method.

---

## Project context for the implementer

You are working in a Helix editor fork at `/Users/angm/helix` on branch `nightly`. Phase 1 is complete (commits `2394f46a6..c00ab544e`). The workspace builds cleanly with `cargo check --workspace` and `cargo build --release -p helix-term --bin hx`.

What Phase 1 left in place:

- `helix-context-schema` workspace crate with snapshot types (`ContextSnapshot`, `Active`, `Cursor`, `Selection`, `Position`, `OpenBuffer`, `Instance`, `UpdateSource`). No protocol types yet.
- `helix-term::context_logger` writes a `ContextSnapshot` to `<workspace>/.helix/context.json` on terminal focus loss, gated by `editor.context-logger.enabled`.
- `helix-view::editor::ContextLoggerConfig` is the config struct shape. `helix-view::editor::Config` has a `context_logger` field.

What you'll add in this phase:

- `helix-context-schema::protocol` module with `ControlRequest::Initialize`, `ControlResponse::Initialize`, `ClientInfo`, `ServerInfo`, `JsonRpcError`.
- `helix-view::editor::ControlSocketConfig` (mirrors `ContextLoggerConfig`'s structure).
- `helix-view::editor::EditorEvent::ControlRequest` variant.
- `helix-term::control_socket` module: path resolution, bind+lifecycle, framing, dispatch, `initialize` handler.
- Wiring in `Application` (startup, event loop, shutdown).

What this phase does NOT do: editor mutation methods, LSP queries, snapshot rewrites on MCP commands, populating the snapshot's `instance` block. Those come in Phase 2b and 2c.

## File structure

**Create:**

- `helix-context-schema/src/protocol.rs` — pure-data JSON-RPC types (`ControlRequest`, `ControlResponse`, `ClientInfo`, `ServerInfo`, `ProtocolVersion`). Roughly 100-150 LOC.
- `helix-context-schema/src/protocol_error.rs` — `JsonRpcError` struct and error code constants. Separate file because it'll grow as later phases add codes. Roughly 60 LOC.
- `helix-context-schema/tests/protocol_roundtrip.rs` — serde round-trip tests for protocol types.
- `helix-term/src/control_socket/mod.rs` — module root, public API (`spawn_server`, `Server` type), `ControlSocketError`.
- `helix-term/src/control_socket/path.rs` — socket path resolution per spec §5.2.
- `helix-term/src/control_socket/lifecycle.rs` — `bind_socket`, orphan check, umask, shutdown unlink.
- `helix-term/src/control_socket/framing.rs` — newline-delimited JSON-RPC reader/writer.
- `helix-term/src/control_socket/dispatch.rs` — request → response routing (just `initialize` for Phase 2a).

**Modify:**

- `Cargo.toml` (workspace) — add `tokio` to workspace.dependencies? No, it's already in `helix-view`'s direct deps. Skip.
- `helix-context-schema/src/lib.rs` — add `pub mod protocol;`, `pub mod protocol_error;`, re-exports.
- `helix-context-schema/Cargo.toml` — no new deps needed (uses existing serde + serde_json).
- `helix-view/Cargo.toml` — add path dep on `helix-context-schema`.
- `helix-view/src/editor.rs` — add `ControlSocketConfig`, `Config::control_socket` field, `EditorEvent::ControlRequest` variant with manual `Debug` impl.
- `helix-term/src/lib.rs` — `pub mod control_socket;`.
- `helix-term/src/application.rs` — start the server in `Application::new` (when enabled), add `tokio::select!` branch in `event_loop_until_idle`, unlink in `close`.

**No `helix-term/Cargo.toml` changes** — tokio + serde + serde_json are already direct deps.

---

## Task 1: Add protocol types to `helix-context-schema`

**Files:**
- Create: `helix-context-schema/src/protocol_error.rs`
- Create: `helix-context-schema/src/protocol.rs`
- Modify: `helix-context-schema/src/lib.rs`
- Create: `helix-context-schema/tests/protocol_roundtrip.rs`

This task adds the protocol surface that this whole phase exists to serve. Doing schema first (before any Helix integration) lets us verify shapes via cheap serde tests before wiring anything.

- [ ] **Step 1: Add `JsonRpcError` (write the failing test first)**

Create `helix-context-schema/tests/protocol_roundtrip.rs`:

```rust
use helix_context_schema::{JsonRpcError, JsonRpcErrorCode};

#[test]
fn json_rpc_error_serializes_with_code_and_message() {
    let err = JsonRpcError {
        code: JsonRpcErrorCode::MethodNotFound,
        message: "Method 'foo' not found".into(),
        data: None,
    };
    let s = serde_json::to_string(&err).unwrap();
    assert!(s.contains(r#""code":-32601"#), "got: {}", s);
    assert!(s.contains(r#""message":"Method 'foo' not found""#), "got: {}", s);
    assert!(!s.contains(r#""data""#), "data should be omitted when None: {}", s);
}

#[test]
fn json_rpc_error_codes_match_jsonrpc_spec() {
    assert_eq!(JsonRpcErrorCode::ParseError as i32, -32700);
    assert_eq!(JsonRpcErrorCode::InvalidRequest as i32, -32600);
    assert_eq!(JsonRpcErrorCode::MethodNotFound as i32, -32601);
    assert_eq!(JsonRpcErrorCode::InvalidParams as i32, -32602);
    assert_eq!(JsonRpcErrorCode::InternalError as i32, -32603);
}

#[test]
fn json_rpc_custom_codes_in_helix_range() {
    assert_eq!(JsonRpcErrorCode::NoLspForLanguage as i32, -32001);
    assert_eq!(JsonRpcErrorCode::LspTimeout as i32, -32002);
    assert_eq!(JsonRpcErrorCode::BufferModeUnsafe as i32, -32003);
    assert_eq!(JsonRpcErrorCode::NoActiveDocument as i32, -32004);
    assert_eq!(JsonRpcErrorCode::PathOutsideWorkspace as i32, -32005);
}
```

- [ ] **Step 2: Run test, confirm it fails**

Run: `cargo test -p helix-context-schema`
Expected: FAIL — `JsonRpcError` not found.

- [ ] **Step 3: Implement `JsonRpcError`**

Create `helix-context-schema/src/protocol_error.rs`:

```rust
//! JSON-RPC 2.0 error types and codes used by the Helix control protocol.
//!
//! Standard codes follow https://www.jsonrpc.org/specification#error_object.
//! Helix-specific codes use the -32000..=-32099 server-error range.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// All error codes the Helix control socket may return. Serialized as the
/// underlying `i32` so wire format matches JSON-RPC 2.0 exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum JsonRpcErrorCode {
    // Standard JSON-RPC 2.0 codes
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,

    // Helix-specific (-32000 to -32099 is JSON-RPC's server-error range)
    NoLspForLanguage = -32001,
    LspTimeout = -32002,
    BufferModeUnsafe = -32003,
    NoActiveDocument = -32004,
    PathOutsideWorkspace = -32005,
}

impl Serialize for JsonRpcErrorCode {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_i32(*self as i32)
    }
}

impl<'de> Deserialize<'de> for JsonRpcErrorCode {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let n = i32::deserialize(de)?;
        match n {
            -32700 => Ok(Self::ParseError),
            -32600 => Ok(Self::InvalidRequest),
            -32601 => Ok(Self::MethodNotFound),
            -32602 => Ok(Self::InvalidParams),
            -32603 => Ok(Self::InternalError),
            -32001 => Ok(Self::NoLspForLanguage),
            -32002 => Ok(Self::LspTimeout),
            -32003 => Ok(Self::BufferModeUnsafe),
            -32004 => Ok(Self::NoActiveDocument),
            -32005 => Ok(Self::PathOutsideWorkspace),
            other => Err(serde::de::Error::custom(format!(
                "unknown JSON-RPC error code {}",
                other
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: JsonRpcErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<Value>,
}
```

- [ ] **Step 4: Wire it into `lib.rs`**

Edit `helix-context-schema/src/lib.rs`. Replace the existing module block (currently `mod snapshot; mod source; mod types;` + pub uses) with:

```rust
//! JSON schema for the Helix context snapshot file (`<workspace>/.helix/context.json`).
//!
//! Used by Helix itself (`helix-term::context_logger`) and by external tools that
//! consume the file (e.g. the planned `helix-claude-mcp` bridge). Schema changes
//! happen here once and surface as compile errors on both producers and consumers.

mod protocol_error;
mod snapshot;
mod source;
mod types;

pub use protocol_error::{JsonRpcError, JsonRpcErrorCode};
pub use snapshot::ContextSnapshot;
pub use source::UpdateSource;
pub use types::{Active, Cursor, Instance, OpenBuffer, Position, Selection};

pub const SCHEMA_VERSION: u32 = 2;
pub const MIN_SUPPORTED_READER: u32 = 1;

/// JSON-RPC protocol version negotiated during the `initialize` handshake.
pub const PROTOCOL_VERSION: &str = "1.0";
```

- [ ] **Step 5: Run tests, confirm error tests pass**

Run: `cargo test -p helix-context-schema`
Expected: 3 new tests pass alongside the existing 11. Total 14.

- [ ] **Step 6: Write tests for `ControlRequest` / `ControlResponse` / `Initialize`**

Append to `helix-context-schema/tests/protocol_roundtrip.rs`:

```rust
use helix_context_schema::{
    ClientInfo, ControlRequest, ControlResponse, ServerCapabilities, ServerInfo,
    PROTOCOL_VERSION,
};

#[test]
fn initialize_request_serializes_with_method_tag() {
    let req = ControlRequest::Initialize {
        protocol_version: PROTOCOL_VERSION.into(),
        client_info: ClientInfo {
            name: "helix-claude-mcp".into(),
            version: "0.1.0".into(),
        },
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "initialize");
    assert_eq!(j["params"]["protocol_version"], "1.0");
    assert_eq!(j["params"]["client_info"]["name"], "helix-claude-mcp");
}

#[test]
fn control_request_deserializes_initialize_from_method_tag() {
    let json = serde_json::json!({
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "test", "version": "0.1.0" }
        }
    });
    let req: ControlRequest = serde_json::from_value(json).unwrap();
    let ControlRequest::Initialize { protocol_version, client_info } = req;
    assert_eq!(protocol_version, "1.0");
    assert_eq!(client_info.name, "test");
}

#[test]
fn initialize_response_round_trips() {
    let resp = ControlResponse::Initialize {
        protocol_version: PROTOCOL_VERSION.into(),
        helix_version: "25.7.1".into(),
        server_info: ServerInfo {
            name: "helix".into(),
            version: "25.7.1".into(),
        },
        capabilities: ServerCapabilities {
            read_methods: vec!["initialize".into()],
            write_methods: vec![],
        },
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["result"]["protocol_version"], "1.0");
    assert_eq!(j["result"]["helix_version"], "25.7.1");
    let back: ControlResponse = serde_json::from_value(j).unwrap();
    let ControlResponse::Initialize { helix_version, .. } = back;
    assert_eq!(helix_version, "25.7.1");
}
```

- [ ] **Step 7: Run, confirm they fail**

Run: `cargo test -p helix-context-schema`
Expected: 3 new tests fail — types not found.

- [ ] **Step 8: Implement protocol types**

Create `helix-context-schema/src/protocol.rs`:

```rust
//! JSON-RPC 2.0 protocol types for the Helix control socket dialect.
//!
//! The wire format is *not* MCP — it's a small custom dialect specific to
//! Helix. An external bridge translates between this and MCP. See spec §6.

use serde::{Deserialize, Serialize};

/// Identification of the client connecting to Helix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Identification of the Helix server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// What this Helix instance can do for clients. The lists are method-name
/// strings (kebab-case, matching the JSON method tags).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub read_methods: Vec<String>,
    pub write_methods: Vec<String>,
}

/// All possible requests the control socket accepts. The wire format uses
/// JSON-RPC 2.0 with `method` and `params` keys; serde's `tag = "method"`
/// generates exactly that shape, and the variant name (kebab-cased) is the
/// method tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "kebab-case")]
pub enum ControlRequest {
    Initialize {
        protocol_version: String,
        client_info: ClientInfo,
    },
}

/// All possible successful responses. The variant name (kebab-cased) matches
/// the request that produced it. Wraps the result payload in a `result` key
/// to mirror JSON-RPC 2.0's response shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "kebab-case")]
pub enum ControlResponse {
    Initialize {
        protocol_version: String,
        helix_version: String,
        server_info: ServerInfo,
        capabilities: ServerCapabilities,
    },
}
```

- [ ] **Step 9: Re-export from `lib.rs`**

Edit `helix-context-schema/src/lib.rs`. Add `mod protocol;` after `mod protocol_error;` (alphabetical-ish) and extend the `pub use` block:

```rust
mod protocol;
mod protocol_error;
mod snapshot;
mod source;
mod types;

pub use protocol::{
    ClientInfo, ControlRequest, ControlResponse, ServerCapabilities, ServerInfo,
};
pub use protocol_error::{JsonRpcError, JsonRpcErrorCode};
pub use snapshot::ContextSnapshot;
pub use source::UpdateSource;
pub use types::{Active, Cursor, Instance, OpenBuffer, Position, Selection};

pub const SCHEMA_VERSION: u32 = 2;
pub const MIN_SUPPORTED_READER: u32 = 1;
pub const PROTOCOL_VERSION: &str = "1.0";
```

- [ ] **Step 10: Run tests, all should pass**

Run: `cargo test -p helix-context-schema`
Expected: 17 tests pass (11 from Phase 1 + 6 new from this task).

- [ ] **Step 11: Commit**

```bash
git add helix-context-schema/src/protocol.rs helix-context-schema/src/protocol_error.rs helix-context-schema/src/lib.rs helix-context-schema/tests/protocol_roundtrip.rs
git commit -m "$(cat <<'EOF'
feat(context-schema): add control-socket JSON-RPC protocol types

ControlRequest and ControlResponse enums tagged by 'method' (JSON-RPC 2.0
shape). Initialize variant only; subsequent methods land in Phase 2b/2c.

JsonRpcError + JsonRpcErrorCode covers the JSON-RPC 2.0 standard codes
plus Helix-specific ones in the -32000..=-32099 server-error range
(NoLspForLanguage, LspTimeout, BufferModeUnsafe, NoActiveDocument,
PathOutsideWorkspace).

PROTOCOL_VERSION = "1.0" exposed as a public const for both producer
(helix-term::control_socket) and future consumer (helix-claude-mcp).

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6)
EOF
)"
```

---

## Task 2: Add `ControlSocketConfig` to `helix-view::Config`

**Files:**
- Modify: `helix-view/Cargo.toml`
- Modify: `helix-view/src/editor.rs`

This task adds the user-facing config keys (`[editor.control-socket]`) and threads the new schema crate into `helix-view`. No socket logic yet — just config plumbing.

- [ ] **Step 1: Add `helix-context-schema` to `helix-view` deps**

Edit `helix-view/Cargo.toml`. Find the `[dependencies]` section. Add (alphabetically among helix-* deps):

```toml
helix-context-schema = { path = "../helix-context-schema" }
```

- [ ] **Step 2: Verify dep wires**

Run: `cargo check -p helix-view`
Expected: Clean (possibly warns about unused dep — that's fine, will be used next).

- [ ] **Step 3: Add `ControlSocketConfig` struct near `ContextLoggerConfig`**

Edit `helix-view/src/editor.rs`. Find the `ContextLoggerConfig` struct (around line 1069 after Phase 1 changes). Immediately after its `impl Default for ContextLoggerConfig`, insert:

```rust
/// Configuration for the control socket (Phase 2+).
///
/// When enabled, Helix binds a Unix-domain socket at `<workspace>/.helix/control-<pid>.sock`
/// and accepts JSON-RPC requests from external tools. Disabled by default —
/// the feature is opt-in.
///
/// See `docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md` (§5) for the
/// full design.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct ControlSocketConfig {
    /// Whether the control socket is enabled. Defaults to false.
    pub enabled: bool,
    /// Override the socket path. Empty string = auto-resolve via
    /// `<workspace>/.helix/control-<pid>.sock` (with macOS pointer-file fallback
    /// for paths that exceed `sun_path` length). See spec §5.2.
    pub path: PathBuf,
}

impl Default for ControlSocketConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: PathBuf::new(),
        }
    }
}
```

- [ ] **Step 4: Add the field to `Config`**

In the same file, find the `pub struct Config` definition (around line 331). Find the `pub context_logger: ContextLoggerConfig,` field added in Phase 0/1. Immediately after it, add:

```rust
    /// Control-socket configuration. Disabled by default. See spec §5.
    #[serde(default)]
    pub control_socket: ControlSocketConfig,
```

- [ ] **Step 5: Add the default initialization**

Find `impl Default for Config` (around line 1192). Find `context_logger: ContextLoggerConfig::default(),`. Immediately after it, add:

```rust
            control_socket: ControlSocketConfig::default(),
```

- [ ] **Step 6: Verify it builds**

Run: `cargo check --workspace`
Expected: Clean.

- [ ] **Step 7: Verify config parses (test by inspection — no automated config test exists)**

Run: `cat <<'EOF' > /tmp/hx-test-cfg.toml
[editor.control-socket]
enabled = true
path = ""
EOF
HELIX_CONFIG=/tmp/hx-test-cfg.toml /Users/angm/helix/target/release/hx --health 2>&1 | head -5
rm /tmp/hx-test-cfg.toml`

(Note: HELIX_CONFIG may not be honored — Helix uses the standard config dir. If --health succeeds without complaining about unknown keys, that's good enough proof the parser accepts the new section.)

- [ ] **Step 8: Commit**

```bash
git add helix-view/Cargo.toml helix-view/src/editor.rs
git commit -m "$(cat <<'EOF'
feat(view): add ControlSocketConfig for the upcoming JSON-RPC socket

Mirrors ContextLoggerConfig in shape and default-disabled posture.
Brings the helix-context-schema crate into helix-view's compile graph
as a workspace path dep (zero new transitive deps — just serde,
serde_json which helix-view already has).

No socket logic yet. Just the config keys: enabled + path.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§5.1)
EOF
)"
```

---

## Task 3: Add `EditorEvent::ControlRequest` variant with manual `Debug` impl

**Files:**
- Modify: `helix-view/src/editor.rs`

The variant carries a `tokio::sync::oneshot::Sender<ControlResponse>` which does not implement `Debug` cleanly. We can't keep `#[derive(Debug)]` on `EditorEvent`. We replace the derive with a hand-written `impl Debug`.

- [ ] **Step 1: Find the `EditorEvent` enum**

Run: `grep -n "pub enum EditorEvent" /Users/angm/helix/helix-view/src/editor.rs`
Expected: One match around line 1428.

- [ ] **Step 2: Read the surrounding context**

Read 20 lines starting from the line returned by Step 1. Confirm the enum has `#[derive(Debug)]` immediately above it and these 6 variants: `DocumentSaved`, `ConfigEvent`, `LanguageServerMessage`, `DebuggerEvent`, `IdleTimer`, `Redraw`.

- [ ] **Step 3: Modify the enum**

Replace the `#[derive(Debug)]` line and the `pub enum EditorEvent { ... }` block with:

```rust
pub enum EditorEvent {
    DocumentSaved(DocumentSavedEventResult),
    ConfigEvent(ConfigEvent),
    LanguageServerMessage((LanguageServerId, Call)),
    DebuggerEvent((DebugAdapterId, dap::Payload)),
    IdleTimer,
    Redraw,
    /// A JSON-RPC request arrived on the control socket. The `reply` channel
    /// must be used exactly once — either with a `ControlResponse` or by
    /// being dropped (which surfaces as `RecvError::Closed` on the sender
    /// side and is mapped to `JsonRpcErrorCode::InternalError`).
    ControlRequest {
        request: helix_context_schema::ControlRequest,
        reply: tokio::sync::oneshot::Sender<
            Result<helix_context_schema::ControlResponse, helix_context_schema::JsonRpcError>,
        >,
    },
}

impl std::fmt::Debug for EditorEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DocumentSaved(v) => f.debug_tuple("DocumentSaved").field(v).finish(),
            Self::ConfigEvent(v) => f.debug_tuple("ConfigEvent").field(v).finish(),
            Self::LanguageServerMessage(v) => {
                f.debug_tuple("LanguageServerMessage").field(v).finish()
            }
            Self::DebuggerEvent(v) => f.debug_tuple("DebuggerEvent").field(v).finish(),
            Self::IdleTimer => f.write_str("IdleTimer"),
            Self::Redraw => f.write_str("Redraw"),
            Self::ControlRequest { request, .. } => f
                .debug_struct("ControlRequest")
                .field("request", request)
                .field("reply", &"<oneshot::Sender>")
                .finish(),
        }
    }
}
```

- [ ] **Step 4: Verify it builds**

Run: `cargo check --workspace`
Expected: Clean. (Both `helix_context_schema::ControlRequest` and `tokio::sync::oneshot::Sender` must be accessible — `tokio` is already in `helix-view`'s deps; `helix-context-schema` was added in Task 2.)

- [ ] **Step 5: Smoke-test the Debug impl**

Add a small test inside `helix-view/src/editor.rs` near other tests (or create one). Find an existing `#[cfg(test)] mod tests { ... }` block in the file; if none exists in this file, skip this step.

Actually, this is a quick sanity check that doesn't need a permanent test. Just run:

```bash
cargo build -p helix-view
```

Expected: Clean compile. The Debug impl is exercised at every `dbg!` and `println!("{:?}", ...)` site in the rest of the codebase, so the workspace check would have caught any breakage.

- [ ] **Step 6: Commit**

```bash
git add helix-view/src/editor.rs
git commit -m "$(cat <<'EOF'
feat(view): add EditorEvent::ControlRequest variant

Carries a typed JSON-RPC request and a oneshot reply channel so per-
connection tokio tasks in helix-term::control_socket can forward
requests into the main event loop (where &mut Editor is available)
and await responses asynchronously.

oneshot::Sender doesn't implement Debug, so the derive(Debug) on
EditorEvent is replaced with a hand-written impl. The new variant
prints as ControlRequest { request: ..., reply: <oneshot::Sender> }.

No new external dep — tokio is already a direct dep of helix-view.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§5.4)
EOF
)"
```

---

## Task 4: Create the `control_socket` module skeleton + path resolution

**Files:**
- Create: `helix-term/src/control_socket/mod.rs`
- Create: `helix-term/src/control_socket/path.rs`
- Modify: `helix-term/src/lib.rs`

- [ ] **Step 1: Write the failing tests for path resolution**

Create `helix-term/src/control_socket/path.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn project_local_path_is_used_when_short_enough() {
        let workspace = PathBuf::from("/repo");
        let pid = 12345;
        let resolved = resolve_socket_path(&workspace, pid, None).unwrap();
        assert_eq!(resolved.primary, PathBuf::from("/repo/.helix/control-12345.sock"));
        assert!(resolved.pointer_target.is_none());
    }

    #[test]
    fn override_path_wins() {
        let workspace = PathBuf::from("/repo");
        let pid = 12345;
        let override_path = PathBuf::from("/custom/where.sock");
        let resolved = resolve_socket_path(&workspace, pid, Some(&override_path)).unwrap();
        assert_eq!(resolved.primary, override_path);
        assert!(resolved.pointer_target.is_none());
    }

    #[test]
    fn long_workspace_path_falls_back_to_runtime_dir() {
        // Construct a path that, with the .helix/control-<pid>.sock suffix,
        // exceeds the 104-byte macOS sun_path limit.
        let long = "/very/long/path".repeat(20);
        let workspace = PathBuf::from(&long);
        let pid = 12345;
        let resolved = resolve_socket_path(&workspace, pid, None).unwrap();
        // Primary is the project-local pointer file
        assert!(resolved.primary.to_string_lossy().ends_with(".sock.path"));
        // Pointer target is the actual runtime-dir socket
        let target = resolved.pointer_target.expect("expected pointer target");
        assert!(target.to_string_lossy().contains("helix"));
        assert!(target.to_string_lossy().ends_with(".sock"));
        // Target itself is under the platform sun_path limit.
        assert!(target.as_os_str().len() <= 104);
    }
}
```

- [ ] **Step 2: Create the module file with stub**

The test above references `resolve_socket_path` and a `Resolved` struct. Add stubs above the `#[cfg(test)]` block:

```rust
//! Resolves the control socket path per spec §5.2.
//!
//! Priority:
//! 1. Explicit override from config
//! 2. `<workspace>/.helix/control-<pid>.sock` if its byte length fits in sun_path
//! 3. Runtime-dir fallback (with project-local pointer file)

use std::io;
use std::path::{Path, PathBuf};

/// Maximum length of a sockaddr_un.sun_path on the most restrictive supported
/// platform (macOS). Linux allows 108. We use the smaller value to keep paths
/// portable.
const MAX_SUN_PATH: usize = 104;

#[derive(Debug)]
pub struct Resolved {
    /// The path Helix advertises to discovery (always inside the workspace
    /// `.helix/` directory). When pointer_target is None, this is also where
    /// the socket itself lives.
    pub primary: PathBuf,
    /// When the project-local path would exceed sun_path, this is the
    /// runtime-dir path the socket actually lives at. The primary path then
    /// points at a tiny text file containing this path, so discoverers can
    /// still find us.
    pub pointer_target: Option<PathBuf>,
}

pub fn resolve_socket_path(
    workspace: &Path,
    pid: u32,
    override_path: Option<&Path>,
) -> io::Result<Resolved> {
    todo!()
}
```

- [ ] **Step 3: Run tests, confirm they fail**

Run: `cargo test -p helix-term control_socket::path`

Wait — this test won't be discoverable yet because the module isn't wired in. Skip to Step 4, then come back.

- [ ] **Step 4: Create `control_socket/mod.rs`**

Create `helix-term/src/control_socket/mod.rs`:

```rust
//! Unix-domain JSON-RPC control socket for external tools (e.g. the
//! helix-claude-mcp bridge). See spec §5-§6.

pub mod path;
```

- [ ] **Step 5: Wire `control_socket` into `helix-term/src/lib.rs`**

Find the `pub mod context_logger;` line. Immediately after it, add:

```rust
pub mod control_socket;
```

- [ ] **Step 6: Run the failing tests**

Run: `cargo test -p helix-term control_socket::path::tests`
Expected: 3 tests, all fail with `not yet implemented` panic.

- [ ] **Step 7: Implement `resolve_socket_path`**

Edit `helix-term/src/control_socket/path.rs`. Replace the `todo!()` body with:

```rust
pub fn resolve_socket_path(
    workspace: &Path,
    pid: u32,
    override_path: Option<&Path>,
) -> io::Result<Resolved> {
    if let Some(p) = override_path {
        return Ok(Resolved {
            primary: p.to_path_buf(),
            pointer_target: None,
        });
    }

    let project_local = workspace
        .join(".helix")
        .join(format!("control-{}.sock", pid));

    if project_local.as_os_str().len() <= MAX_SUN_PATH {
        return Ok(Resolved {
            primary: project_local,
            pointer_target: None,
        });
    }

    // Project path too long — fall back to runtime dir with a pointer file.
    let runtime_socket = runtime_socket_path(workspace, pid)?;
    let pointer = workspace
        .join(".helix")
        .join(format!("control-{}.sock.path", pid));

    Ok(Resolved {
        primary: pointer,
        pointer_target: Some(runtime_socket),
    })
}

/// Picks a per-user directory that's likely to fit within sun_path limits
/// and survive across crashes (not /tmp, which is world-writable).
fn runtime_socket_path(workspace: &Path, pid: u32) -> io::Result<PathBuf> {
    let base = if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    } else {
        dirs::cache_dir().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no cache dir for runtime socket")
        })?
    };

    // Hash the workspace path so multiple Helix instances in different
    // workspaces don't collide in this shared directory.
    let workspace_hash = simple_hash(workspace.as_os_str().as_encoded_bytes());
    Ok(base
        .join("helix")
        .join(format!("control-{}-{:x}.sock", pid, workspace_hash)))
}

/// FNV-1a 64-bit. We don't need cryptographic quality — just stability
/// across runs so the pointer file always points at the same fallback path
/// for a given workspace.
fn simple_hash(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
```

- [ ] **Step 8: Add `dirs` dep**

The `dirs` crate is not yet a `helix-term` dependency. Check first:

Run: `grep -n '^dirs' /Users/angm/helix/helix-term/Cargo.toml`

If no match, add to `helix-term/Cargo.toml` `[dependencies]`:

```toml
dirs = "5.0"
```

If `dirs` is already present, skip.

- [ ] **Step 9: Run tests**

Run: `cargo test -p helix-term control_socket::path::tests`
Expected: 3 tests pass.

- [ ] **Step 10: Commit**

```bash
git add helix-term/src/control_socket/ helix-term/src/lib.rs helix-term/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(control-socket): path resolution module

Resolves the socket path per spec §5.2:
1. Explicit override from config
2. <workspace>/.helix/control-<pid>.sock when its length fits
3. Runtime-dir fallback ($XDG_RUNTIME_DIR or $TMPDIR/cache_dir) with a
   pointer file at the project-local path containing the real socket path
   — for paths that exceed the platform's sun_path limit (104 bytes on
   macOS).

Workspace path is hashed (FNV-1a 64-bit) into the fallback filename so
multiple Helix instances in different workspaces don't collide in the
shared runtime dir.

Tested with project-local, override, and long-path cases.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§5.2)
EOF
)"
```

---

## Task 5: Implement socket lifecycle (bind, umask, chmod, orphan check, cleanup)

**Files:**
- Create: `helix-term/src/control_socket/lifecycle.rs`
- Modify: `helix-term/src/control_socket/mod.rs`

- [ ] **Step 1: Write the failing tests**

Create `helix-term/src/control_socket/lifecycle.rs`:

```rust
//! Socket bind + lifecycle per spec §5.3.

use std::io;
use std::path::Path;
use tokio::net::UnixListener;

use crate::control_socket::path::Resolved;

pub struct Binding {
    pub listener: UnixListener,
    pub resolved: Resolved,
}

/// Bind the control socket. Handles orphan cleanup, umask for atomic 0600
/// mode, and writes the pointer file if the path resolution required one.
pub fn bind_socket(resolved: Resolved) -> io::Result<Binding> {
    todo!()
}

/// Unlink everything bind_socket created (socket file + optional pointer
/// file). Called from Application::close.
pub fn unlink(resolved: &Resolved) -> io::Result<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn bind_then_unlink_leaves_no_files() {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");
        let resolved = Resolved {
            primary: socket_path.clone(),
            pointer_target: None,
        };
        let binding = bind_socket(resolved).unwrap();
        assert!(socket_path.exists(), "socket file should exist after bind");
        drop(binding.listener);
        unlink(&Resolved { primary: socket_path.clone(), pointer_target: None }).unwrap();
        assert!(!socket_path.exists(), "socket file should be gone after unlink");
    }

    #[test]
    fn bind_unlinks_existing_orphan_socket_file() {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("orphan.sock");
        // Pre-create a file at the socket path to simulate an orphan from a
        // crashed prior process. This isn't a real socket — connect() will
        // fail — so bind should unlink it and proceed.
        std::fs::File::create(&socket_path).unwrap();
        let resolved = Resolved {
            primary: socket_path.clone(),
            pointer_target: None,
        };
        let binding = bind_socket(resolved).unwrap();
        assert!(socket_path.exists(), "new socket should be bound at the same path");
        drop(binding.listener);
        unlink(&Resolved { primary: socket_path, pointer_target: None }).ok();
    }

    #[test]
    fn pointer_file_is_written_when_resolved_has_target() {
        let tmp = TempDir::new().unwrap();
        let pointer = tmp.path().join("pointer.sock.path");
        let actual = tmp.path().join("real.sock");
        let resolved = Resolved {
            primary: pointer.clone(),
            pointer_target: Some(actual.clone()),
        };
        let binding = bind_socket(resolved).unwrap();
        assert!(pointer.exists(), "pointer file should exist");
        assert!(actual.exists(), "real socket file should exist");
        let pointer_contents = std::fs::read_to_string(&pointer).unwrap();
        assert_eq!(pointer_contents.trim(), actual.to_string_lossy());
        drop(binding.listener);
        unlink(&Resolved { primary: pointer.clone(), pointer_target: Some(actual.clone()) }).ok();
        assert!(!pointer.exists());
        assert!(!actual.exists());
    }
}
```

- [ ] **Step 2: Wire the new module into `mod.rs`**

Edit `helix-term/src/control_socket/mod.rs`:

```rust
//! Unix-domain JSON-RPC control socket for external tools (e.g. the
//! helix-claude-mcp bridge). See spec §5-§6.

pub mod lifecycle;
pub mod path;
```

- [ ] **Step 3: Add `tempfile` to `[dev-dependencies]` if not present**

Run: `grep -n "tempfile" /Users/angm/helix/helix-term/Cargo.toml`

If `tempfile` is already used as a regular dep (likely — it's in `[workspace.dependencies]`), it's accessible to tests via `[dev-dependencies]` or directly. If absent, add to `helix-term/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = { workspace = true }
```

- [ ] **Step 4: Run the failing tests**

Run: `cargo test -p helix-term control_socket::lifecycle::tests`
Expected: 3 tests, all panic at `todo!()`.

- [ ] **Step 5: Implement `bind_socket` and `unlink`**

Replace the `todo!()` bodies in `helix-term/src/control_socket/lifecycle.rs`:

```rust
pub fn bind_socket(resolved: Resolved) -> io::Result<Binding> {
    let bind_path: &Path = resolved
        .pointer_target
        .as_deref()
        .unwrap_or(&resolved.primary);

    // Ensure the parent directory exists with mode 0700 if we create it.
    if let Some(parent) = bind_path.parent() {
        std::fs::create_dir_all(parent)?;
        // Best-effort: tighten permissions on the parent dir we may have created.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            // Ignore error: parent might be a shared dir (e.g. .helix) we can't chmod.
            let _ = std::fs::set_permissions(parent, perms);
        }
    }

    // If a stale file is at the bind path, check whether it's a live socket
    // before unlinking. Live socket means another instance owns it — we don't
    // touch it, just error out. Stale means we unlink and proceed.
    if bind_path.exists() {
        if is_socket_live(bind_path) {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "control socket {} is already owned by a live process",
                    bind_path.display()
                ),
            ));
        }
        std::fs::remove_file(bind_path)?;
    }

    // Atomically bind with mode 0600 by wrapping the bind in a tighter umask.
    let listener = with_strict_umask(|| UnixListener::bind(bind_path))?;

    // Belt-and-suspenders: explicit chmod even though the umask should have
    // done it. Some filesystems (e.g. ones with default ACLs) may override
    // umask.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(bind_path, perms)?;
    }

    // If we used the runtime-dir fallback, write the pointer file in the
    // project's .helix/ dir so external discoverers can still find us.
    if resolved.pointer_target.is_some() {
        let primary = &resolved.primary;
        if let Some(parent) = primary.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(primary, bind_path.to_string_lossy().as_bytes())?;
    }

    Ok(Binding { listener, resolved })
}

pub fn unlink(resolved: &Resolved) -> io::Result<()> {
    let bind_path: &Path = resolved
        .pointer_target
        .as_deref()
        .unwrap_or(&resolved.primary);
    if bind_path.exists() {
        std::fs::remove_file(bind_path)?;
    }
    if resolved.pointer_target.is_some() && resolved.primary.exists() {
        std::fs::remove_file(&resolved.primary)?;
    }
    Ok(())
}

/// Try to connect() to the socket path. If connect succeeds, something is
/// listening and we must NOT touch the file. If connect fails with
/// ECONNREFUSED or ENOENT, it's stale and can be unlinked.
fn is_socket_live(path: &Path) -> bool {
    use std::os::unix::net::UnixStream;
    match UnixStream::connect(path) {
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Temporarily set umask to 0o077 so the bound socket gets mode 0600
/// atomically with creation, then restore.
fn with_strict_umask<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    #[cfg(unix)]
    {
        // SAFETY: umask is process-global. This is fine for a foreground
        // editor process whose threads aren't creating files concurrently
        // during the brief bind window. If Helix ever grows concurrent file
        // creation here, switch to a per-fd approach (e.g. socketpair +
        // bind by syscall with explicit mode).
        let prev = unsafe { libc::umask(0o077) };
        let out = f();
        unsafe {
            libc::umask(prev);
        }
        out
    }
    #[cfg(not(unix))]
    {
        f()
    }
}
```

- [ ] **Step 6: Confirm `libc` is available**

Run: `grep -n '^libc' /Users/angm/helix/helix-term/Cargo.toml`

If present: good. If not, add to `[dependencies]`:

```toml
libc = "0.2"
```

(`libc` is almost certainly already in the dep graph; it's a transitive dep of tokio. Adding it as a direct dep makes the use explicit.)

- [ ] **Step 7: Run tests**

Run: `cargo test -p helix-term control_socket::lifecycle::tests`
Expected: 3 tests pass.

- [ ] **Step 8: Commit**

```bash
git add helix-term/src/control_socket/lifecycle.rs helix-term/src/control_socket/mod.rs helix-term/Cargo.toml
git commit -m "$(cat <<'EOF'
feat(control-socket): bind/unlink lifecycle

Implements spec §5.3 socket lifecycle:
- Detect orphaned vs live sockets via UnixStream::connect on the path
  before bind; unlink stale, refuse to clobber live
- umask 0o077 around bind for atomic 0600 mode; explicit chmod after
  as belt-and-suspenders for filesystems with default ACLs
- Pointer file written in <workspace>/.helix/ when the actual socket
  lives in the runtime dir (macOS sun_path overflow case)
- unlink() cleanly removes both socket and (if used) pointer file

Three tests cover normal bind+unlink, orphan-replacement, and pointer-file
write-and-cleanup.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§5.3)
EOF
)"
```

---

## Task 6: JSON-RPC newline-delimited framing

**Files:**
- Create: `helix-term/src/control_socket/framing.rs`
- Modify: `helix-term/src/control_socket/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `helix-term/src/control_socket/framing.rs`:

```rust
//! Newline-delimited JSON framing over async streams. One JSON object per
//! line, separated by a single `\n`. Lines longer than `MAX_FRAME_BYTES`
//! produce a framing error (defensive against malformed input).

use std::io;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use helix_context_schema::{ControlRequest, ControlResponse, JsonRpcError};

const MAX_FRAME_BYTES: usize = 1024 * 1024; // 1 MiB

pub struct FrameReader {
    inner: BufReader<OwnedReadHalf>,
    buf: String,
}

impl FrameReader {
    pub fn new(half: OwnedReadHalf) -> Self {
        Self {
            inner: BufReader::new(half),
            buf: String::new(),
        }
    }

    /// Read one JSON-RPC frame, parsed into ControlRequest. Returns None on
    /// EOF (peer closed). Errors on malformed JSON or oversize frame.
    pub async fn read_request(&mut self) -> io::Result<Option<ControlRequest>> {
        self.buf.clear();
        let n = self.inner.read_line(&mut self.buf).await?;
        if n == 0 {
            return Ok(None); // EOF
        }
        if self.buf.len() > MAX_FRAME_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
        }
        let trimmed = self.buf.trim_end_matches(['\r', '\n']);
        let req: ControlRequest = serde_json::from_str(trimmed)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(req))
    }
}

pub struct FrameWriter {
    inner: OwnedWriteHalf,
}

impl FrameWriter {
    pub fn new(half: OwnedWriteHalf) -> Self {
        Self { inner: half }
    }

    pub async fn write_response(
        &mut self,
        resp: &Result<ControlResponse, JsonRpcError>,
    ) -> io::Result<()> {
        let mut bytes = match resp {
            Ok(r) => serde_json::to_vec(r),
            Err(e) => serde_json::to_vec(e),
        }
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        bytes.push(b'\n');
        self.inner.write_all(&bytes).await?;
        self.inner.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_context_schema::{ClientInfo, ControlRequest};
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn round_trip_initialize_request() {
        let (a, b) = UnixStream::pair().unwrap();
        let (a_read, _a_write) = a.into_split();
        let (_b_read, mut b_write) = b.into_split();

        // Send a request frame over `b_write`
        let payload =
            br#"{"method":"initialize","params":{"protocol_version":"1.0","client_info":{"name":"test","version":"0.1"}}}"#;
        b_write.write_all(payload).await.unwrap();
        b_write.write_all(b"\n").await.unwrap();
        b_write.flush().await.unwrap();
        drop(b_write); // Close write side so EOF is reachable

        // Read it on the other side
        let mut reader = FrameReader::new(a_read);
        let req = reader.read_request().await.unwrap().expect("expected a request");
        let ControlRequest::Initialize { protocol_version, client_info } = req;
        assert_eq!(protocol_version, "1.0");
        assert_eq!(client_info.name, "test");

        // EOF reaches us next
        let eof = reader.read_request().await.unwrap();
        assert!(eof.is_none());
    }

    #[tokio::test]
    async fn malformed_json_returns_invalid_data_error() {
        let (a, b) = UnixStream::pair().unwrap();
        let (a_read, _) = a.into_split();
        let (_, mut b_write) = b.into_split();

        b_write.write_all(b"not json at all\n").await.unwrap();
        b_write.flush().await.unwrap();

        let mut reader = FrameReader::new(a_read);
        let err = reader.read_request().await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
```

- [ ] **Step 2: Wire `framing` into `mod.rs`**

Edit `helix-term/src/control_socket/mod.rs`:

```rust
//! Unix-domain JSON-RPC control socket for external tools (e.g. the
//! helix-claude-mcp bridge). See spec §5-§6.

pub mod framing;
pub mod lifecycle;
pub mod path;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p helix-term control_socket::framing::tests`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add helix-term/src/control_socket/framing.rs helix-term/src/control_socket/mod.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): newline-delimited JSON-RPC framing

FrameReader and FrameWriter over UnixStream's owned halves. One JSON
object per line. 1 MiB frame size cap to defend against pathological
input. Malformed JSON surfaces as io::ErrorKind::InvalidData.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.1)
EOF
)"
```

---

## Task 7: Per-connection task + dispatch + `initialize` handler

**Files:**
- Create: `helix-term/src/control_socket/dispatch.rs`
- Modify: `helix-term/src/control_socket/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `helix-term/src/control_socket/dispatch.rs`:

```rust
//! Request → response dispatch. For Phase 2a, only the `initialize` method
//! is implemented; subsequent phases extend the match.

use helix_context_schema::{
    ClientInfo, ControlRequest, ControlResponse, JsonRpcError, JsonRpcErrorCode,
    ServerCapabilities, ServerInfo, PROTOCOL_VERSION,
};

/// Methods that don't require entering the editor event loop (no
/// `&mut Editor`). Currently just `initialize`. Returning `Ok(Some(resp))`
/// means we handled it inline; `Ok(None)` means it must be forwarded to
/// the main loop via `EditorEvent::ControlRequest`.
pub fn try_dispatch_inline(
    request: &ControlRequest,
) -> Option<Result<ControlResponse, JsonRpcError>> {
    match request {
        ControlRequest::Initialize {
            protocol_version,
            client_info,
        } => Some(handle_initialize(protocol_version, client_info)),
    }
}

fn handle_initialize(
    client_protocol_version: &str,
    _client_info: &ClientInfo,
) -> Result<ControlResponse, JsonRpcError> {
    if !is_compatible_protocol(client_protocol_version, PROTOCOL_VERSION) {
        return Err(JsonRpcError {
            code: JsonRpcErrorCode::InvalidParams,
            message: format!(
                "client protocol version {} is incompatible with server {}",
                client_protocol_version, PROTOCOL_VERSION
            ),
            data: None,
        });
    }
    Ok(ControlResponse::Initialize {
        protocol_version: PROTOCOL_VERSION.into(),
        helix_version: env!("CARGO_PKG_VERSION").into(),
        server_info: ServerInfo {
            name: "helix".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        capabilities: ServerCapabilities {
            read_methods: vec!["initialize".into()],
            write_methods: vec![],
        },
    })
}

/// Same major version means compatible. "1.0" ↔ "1.5" OK; "1.0" ↔ "2.0" not.
fn is_compatible_protocol(client: &str, server: &str) -> bool {
    let major = |s: &str| -> Option<u32> { s.split('.').next()?.parse().ok() };
    match (major(client), major(server)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_compatible_version_returns_ok() {
        let req = ControlRequest::Initialize {
            protocol_version: "1.0".into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let resp = try_dispatch_inline(&req).unwrap().unwrap();
        let ControlResponse::Initialize { capabilities, .. } = resp;
        assert!(capabilities.read_methods.contains(&"initialize".to_string()));
    }

    #[test]
    fn initialize_incompatible_major_version_returns_invalid_params() {
        let req = ControlRequest::Initialize {
            protocol_version: "2.0".into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let err = try_dispatch_inline(&req).unwrap().unwrap_err();
        assert_eq!(err.code, JsonRpcErrorCode::InvalidParams);
    }

    #[test]
    fn major_version_compatibility_check() {
        assert!(is_compatible_protocol("1.0", "1.0"));
        assert!(is_compatible_protocol("1.5", "1.0"));
        assert!(is_compatible_protocol("1.0", "1.5"));
        assert!(!is_compatible_protocol("2.0", "1.0"));
        assert!(!is_compatible_protocol("", "1.0"));
        assert!(!is_compatible_protocol("garbage", "1.0"));
    }
}
```

- [ ] **Step 2: Wire dispatch into `mod.rs`**

Edit `helix-term/src/control_socket/mod.rs`:

```rust
//! Unix-domain JSON-RPC control socket for external tools (e.g. the
//! helix-claude-mcp bridge). See spec §5-§6.

pub mod dispatch;
pub mod framing;
pub mod lifecycle;
pub mod path;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p helix-term control_socket::dispatch::tests`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add helix-term/src/control_socket/dispatch.rs helix-term/src/control_socket/mod.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): dispatch + initialize handler

try_dispatch_inline handles methods that don't need editor state. For
Phase 2a that's just `initialize`. Returns None for methods that must
be forwarded to the main event loop (none in this phase, but the
signature is ready for Phase 2b/2c).

Initialize negotiates protocol version (major-version compatibility:
1.x clients accepted, 2.x rejected) and advertises currently-supported
methods. helix_version comes from CARGO_PKG_VERSION at compile time.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.1)
EOF
)"
```

---

## Task 8: Connection task + server entry point

**Files:**
- Create: `helix-term/src/control_socket/server.rs`
- Modify: `helix-term/src/control_socket/mod.rs`

This is the glue: the function that accepts connections on a listener and spawns a per-connection task that loops on read_request → dispatch → write_response.

- [ ] **Step 1: Write the failing integration test**

Create `helix-term/src/control_socket/server.rs`:

```rust
//! Spawns the per-connection tasks that read requests, dispatch them, and
//! write responses. The outer accept loop runs as a tokio task spawned by
//! `Application::start_control_server`.

use std::io;
use tokio::net::UnixListener;

use crate::control_socket::dispatch::try_dispatch_inline;
use crate::control_socket::framing::{FrameReader, FrameWriter};

/// Accept connections forever and spawn a per-connection task for each.
/// Returns when the listener is dropped (which happens on Application::close).
pub async fn run_accept_loop(listener: UnixListener) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                tokio::spawn(handle_connection(stream));
            }
            Err(e) => {
                log::warn!("control-socket: accept failed: {}", e);
                // Brief backoff to avoid a hot loop if accept is failing.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle_connection(stream: tokio::net::UnixStream) {
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);

    loop {
        let req = match reader.read_request().await {
            Ok(Some(req)) => req,
            Ok(None) => break, // EOF — peer closed
            Err(e) => {
                log::warn!("control-socket: frame read error: {}", e);
                let _ = writer
                    .write_response(&Err(
                        helix_context_schema::JsonRpcError {
                            code: helix_context_schema::JsonRpcErrorCode::ParseError,
                            message: format!("{}", e),
                            data: None,
                        },
                    ))
                    .await;
                break;
            }
        };

        let resp = match try_dispatch_inline(&req) {
            Some(resp) => resp,
            None => Err(helix_context_schema::JsonRpcError {
                code: helix_context_schema::JsonRpcErrorCode::MethodNotFound,
                message: "method not available in Phase 2a".into(),
                data: None,
            }),
        };

        if let Err(e) = writer.write_response(&resp).await {
            log::warn!("control-socket: write error: {}", e);
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_socket::lifecycle::{bind_socket, unlink};
    use crate::control_socket::path::Resolved;
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn client_can_complete_initialize_handshake() {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("ipc.sock");
        let resolved = Resolved {
            primary: socket_path.clone(),
            pointer_target: None,
        };
        let binding = bind_socket(resolved).unwrap();

        let server = tokio::spawn(run_accept_loop(binding.listener));

        // Client side
        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let request =
            br#"{"method":"initialize","params":{"protocol_version":"1.0","client_info":{"name":"test","version":"0.1"}}}"#;
        client.write_all(request).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.flush().await.unwrap();

        let (read_half, _) = client.into_split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains(r#""method":"initialize""#));
        assert!(line.contains(r#""protocol_version":"1.0""#));
        assert!(line.contains(r#""helix_version""#));

        server.abort();
        unlink(&Resolved { primary: socket_path, pointer_target: None }).ok();
    }
}
```

- [ ] **Step 2: Wire `server` into `mod.rs`**

Edit `helix-term/src/control_socket/mod.rs`:

```rust
//! Unix-domain JSON-RPC control socket for external tools (e.g. the
//! helix-claude-mcp bridge). See spec §5-§6.

pub mod dispatch;
pub mod framing;
pub mod lifecycle;
pub mod path;
pub mod server;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p helix-term control_socket::server::tests`
Expected: 1 test passes (the end-to-end handshake).

- [ ] **Step 4: Run all control_socket tests together**

Run: `cargo test -p helix-term control_socket`
Expected: All tests across path/lifecycle/framing/dispatch/server pass (12 total).

- [ ] **Step 5: Commit**

```bash
git add helix-term/src/control_socket/server.rs helix-term/src/control_socket/mod.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): per-connection task + accept loop

run_accept_loop spawns a handle_connection task per incoming connection.
Each connection's loop reads frames, dispatches via try_dispatch_inline,
writes responses. EOF cleanly exits the loop. Frame parse errors send
a ParseError response and close. Methods not yet implemented surface as
MethodNotFound.

End-to-end test: bind socket, start accept loop, connect a client, send
initialize over UnixStream, parse response, confirm version negotiation.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6)
EOF
)"
```

---

## Task 9: Wire socket startup, event loop, and cleanup into `Application`

**Files:**
- Modify: `helix-term/src/application.rs`

This task threads the control socket through the existing `Application` lifecycle. Three integration points: startup (boot the listener if config enables it), event loop (will be exercised in Phase 2b/2c — for now it's a no-op stub), and shutdown (unlink the socket files).

- [ ] **Step 1: Find the constructor**

Run: `grep -n "impl Application" /Users/angm/helix/helix-term/src/application.rs | head -5`

Find the `pub async fn new(...)` or `impl Application { pub fn new(...) }` constructor. Read around it to understand its shape (parameters, what's returned, what fields are initialized).

- [ ] **Step 2: Add a control-socket binding field to `Application`**

Find the `pub struct Application { ... }` definition. Add a field (after the existing fields, before the closing brace):

```rust
    /// Bound when [editor.control-socket] enabled = true. The accept loop
    /// runs as a tokio task; this handle is kept so we can unlink the
    /// socket files on shutdown.
    control_socket_binding: Option<crate::control_socket::lifecycle::Binding>,
```

- [ ] **Step 3: Initialize the field in the constructor**

In `Application::new`, somewhere after `editor` and `compositor` are built but before the constructor returns, add:

```rust
        let control_socket_binding = if editor.config().control_socket.enabled {
            match start_control_socket(&editor) {
                Ok(binding) => Some(binding),
                Err(e) => {
                    log::warn!("control-socket: failed to start: {}", e);
                    None
                }
            }
        } else {
            None
        };
```

Then add `control_socket_binding,` to the `Application { ... }` struct literal returned from `new`.

- [ ] **Step 4: Add `Binding::split()` to lifecycle**

The listener needs to move into the spawned accept-loop task while the resolved path stays with `Application` for shutdown unlink. Add a `split()` method on `Binding` to separate these.

Edit `helix-term/src/control_socket/lifecycle.rs`. After the existing `pub struct Binding { ... }` and the bind/unlink functions, add:

```rust
impl Binding {
    /// Move the listener out, leaving the resolved path behind. Used by
    /// Application::new to spawn the accept loop while keeping the path
    /// for unlink-on-shutdown.
    pub fn split(self) -> (UnixListener, Resolved) {
        (self.listener, self.resolved)
    }
}
```

Run: `cargo check -p helix-term`
Expected: Clean (the method is currently unused — `dead_code` warning is fine).

- [ ] **Step 5: Add `start_control_socket` helper to `application.rs`**

Near the top of `helix-term/src/application.rs` (after the imports), add:

```rust
/// Start the control socket if config enables it. Returns the resolved path
/// so the caller can unlink it on shutdown.
fn start_control_socket(
    editor: &helix_view::Editor,
) -> std::io::Result<crate::control_socket::path::Resolved> {
    use crate::control_socket::{lifecycle, path, server};

    let (workspace, is_cwd_fallback) = helix_loader::find_workspace();
    if is_cwd_fallback {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "no workspace marker — refusing to bind control socket in fallback directory",
        ));
    }

    let cfg = &editor.config().control_socket;
    let override_path = if cfg.path.as_os_str().is_empty() {
        None
    } else {
        Some(cfg.path.as_path())
    };

    let resolved = path::resolve_socket_path(&workspace, std::process::id(), override_path)?;
    let binding = lifecycle::bind_socket(resolved)?;
    let (listener, resolved_for_cleanup) = binding.split();

    tokio::spawn(server::run_accept_loop(listener));

    Ok(resolved_for_cleanup)
}
```

Now update the `Application` field type from earlier in this task. Earlier (in Step 2) we declared:

```rust
    control_socket_binding: Option<crate::control_socket::lifecycle::Binding>,
```

Change it to:

```rust
    control_socket_binding: Option<crate::control_socket::path::Resolved>,
```

And update the initialization in `Application::new` (from Step 3) to use the new helper's return type:

```rust
        let control_socket_binding = if editor.config().control_socket.enabled {
            match start_control_socket(&editor) {
                Ok(resolved) => Some(resolved),
                Err(e) => {
                    log::warn!("control-socket: failed to start: {}", e);
                    None
                }
            }
        } else {
            None
        };
```

Run: `cargo check -p helix-term`
Expected: Clean.

- [ ] **Step 6: Wire shutdown cleanup in `Application::close`**

Find `Application::close`. After existing cleanup logic (jobs.finish, flush_writes, close_language_servers), add:

```rust
        if let Some(resolved) = self.control_socket_binding.take() {
            if let Err(e) = crate::control_socket::lifecycle::unlink(&resolved) {
                log::warn!("control-socket: failed to unlink at shutdown: {}", e);
            }
        }
```

- [ ] **Step 7: Verify the whole workspace builds**

Run: `cargo check --workspace`
Expected: Clean.

- [ ] **Step 8: Verify the existing tests still pass**

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: All tests pass.

- [ ] **Step 9: Build the release binary**

Run: `cargo build --release -p helix-term --bin hx`
Expected: Succeeds. The binary will start the socket if `[editor.control-socket] enabled = true` in the user's config.

- [ ] **Step 10: Manual end-to-end smoke test**

This needs a real, brief Helix run. Set up a test project:

```bash
mkdir -p /tmp/ctrl-test/.helix
cat > /tmp/ctrl-test/.helix/config.toml <<'EOF'
[editor.control-socket]
enabled = true
path = ""

[editor.context-logger]
enabled = false
EOF
mkdir /tmp/ctrl-test/src && echo "fn main() {}" > /tmp/ctrl-test/src/main.rs
cd /tmp/ctrl-test && git init -q
```

Run Helix in the background:

```bash
cd /tmp/ctrl-test && /Users/angm/helix/target/release/hx src/main.rs &
HX_PID=$!
sleep 1
```

Verify the socket exists:

```bash
ls -la /tmp/ctrl-test/.helix/control-*.sock
```

Expected: One socket file with mode `srw-------` (0600) named `control-<HX_PID>.sock`.

Send an initialize over the socket:

```bash
SOCK=$(ls /tmp/ctrl-test/.helix/control-*.sock | head -1)
printf '{"method":"initialize","params":{"protocol_version":"1.0","client_info":{"name":"smoke","version":"0.1"}}}\n' \
    | nc -U "$SOCK" -q 1
```

Expected: A single JSON line containing `"method":"initialize"` and `"protocol_version":"1.0"` and `"helix_version":"..."`.

Tear down:

```bash
kill $HX_PID 2>/dev/null
sleep 0.5
ls /tmp/ctrl-test/.helix/  # Should NOT show any control-*.sock file
rm -rf /tmp/ctrl-test
```

Expected: socket cleaned up by `Application::close`.

If `nc -U` isn't available on macOS (it's a common dependency), use Python instead:

```bash
python3 -c "
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"initialize\",\"params\":{\"protocol_version\":\"1.0\",\"client_info\":{\"name\":\"smoke\",\"version\":\"0.1\"}}}\n')
print(s.recv(4096).decode())
"
```

- [ ] **Step 11: Commit**

```bash
git add helix-term/src/application.rs helix-term/src/control_socket/lifecycle.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): wire startup, event loop integration, and cleanup
into Application

- Application::new starts the socket when [editor.control-socket] enabled
- Spawns a tokio task running run_accept_loop on the listener
- Skips startup if is_cwd_fallback (no workspace marker)
- Application::close unlinks both the socket and any pointer file

Binding::split() lets the listener move into the spawned task while the
resolved path stays with Application for cleanup.

Smoke-tested end-to-end: connect via Python, complete initialize
handshake, verify socket file is mode 0600, confirm clean unlink on
Helix exit.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§5.1, §5.3)
EOF
)"
```

---

## Self-review checklist (for the implementer)

After all 9 tasks:

- [ ] `cargo check --workspace` clean
- [ ] `cargo test -p helix-context-schema` passes (17 tests: 11 prior + 6 new from Task 1)
- [ ] `cargo test -p helix-term control_socket` passes (12 tests across path/lifecycle/framing/dispatch/server)
- [ ] `cargo build --release -p helix-term --bin hx` succeeds
- [ ] Smoke test from Task 9 Step 10 reproduces (socket binds, mode 0600, initialize completes, socket unlinks on Helix exit)
- [ ] `git log --oneline -10` shows the Phase 2a commits in clean sequence

## What's NOT in Phase 2a

- **`current-state`, `get-buffer-text`, `get-open-buffers` methods** — Phase 2b.
- **`open-file`, `goto-line`, any state-mutating methods** — Phase 2c.
- **`EditorEvent::ControlRequest` actually being fired** — Phase 2b will start using this when read methods need editor state.
- **Snapshot `instance` block population** — Phase 2c (needs the socket path written, which by then is stable).
- **LSP-backed methods** — Phase 3.
- **`helix-claude-mcp` external binary** — Phase 4.

If you find yourself implementing any of the above, stop and consult the spec / the next phase plan when it's written.

## Open questions for the implementer

1. **Detection of `EditorEvent::ControlRequest` event flow in Phase 2a.** Task 3 adds the variant but Phase 2a never emits it (since only `initialize` is implemented and it doesn't need editor state). The variant compiles but is unused. This is intentional — Phase 2b will start emitting it. If the Rust compiler warns about an unused variant, suppress with `#[allow(dead_code)]` on the variant or accept the warning until Phase 2b.

2. **What about the `path = ""` default?** Spec §5.1 shows `path = ""` as the default meaning "auto-resolve". This plan implements that by checking `cfg.path.as_os_str().is_empty()`. If the implementer finds this awkward, an alternative is to make `path` an `Option<PathBuf>`. Either is fine for Phase 2a; keep it as `PathBuf` with empty-string sentinel for consistency with how the spec's example config is written.
