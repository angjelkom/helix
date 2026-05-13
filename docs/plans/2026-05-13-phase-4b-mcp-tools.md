# Phase 4b — MCP Tools — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire seven MCP Tools to Helix's control socket via the JSON-RPC client and discovery modules built in Phase 4a. End state: Claude Code can drive Helix end-to-end — open files, jump cursors, query LSP — through MCP `tools/call`.

**Architecture:** Each tool is a thin adapter in `helix-claude-mcp`'s server. `call_tool` dispatches by name: parse args → build `ControlRequest` (using the schema crate's types) → call `discovery::find_helix_socket` to locate a running Helix → call `rpc_client::send_request` to roundtrip → format `ControlResponse` back as a JSON string in MCP's `text` content type. Discovery + RPC are both cached per-call (no connection pooling); a 5-second discovery cache could be added later if call rate justifies it. Errors at any step (no Helix running, Helix returned `JsonRpcError`, RPC timeout, malformed args) surface as MCP tool errors with actionable messages.

**Tech Stack:** Same as Phase 4a — `rmcp` 1.6.0, tokio, serde + serde_json, `helix-context-schema`. No new external deps.

**Spec:** `docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md` §7.3 (Resources vs Tools mapping), §6 (Helix's control protocol that we're bridging).

---

## Project context for the implementer

You are working in a Helix editor fork at `/Users/angm/helix` on branch `nightly`. Phase 4a is complete (tip: `503ca2013`, 63 commits ahead of remote). The `helix-claude-mcp` binary is functional as an MCP server with three Resources backed by the snapshot file. Phase 4a built but did not yet use: `rpc_client::send_request` and `discovery::find_helix_socket`.

**Helix-side methods this phase bridges to** (all implemented in Phases 2c and 3, verified end-to-end):

- Write methods: `open-file`, `goto-line`
- LSP read methods: `get-diagnostics`, `get-hover-at`, `get-definition-at`, `get-references-at`, `get-workspace-symbols`

**MCP Tools this phase exposes** (one per Helix method, names use underscores not dots per Claude Code's stricter tool-name validator):

| MCP Tool name | Helix method | Type |
|---|---|---|
| `helix_open_file` | `open-file` | Write |
| `helix_goto_line` | `goto-line` | Write |
| `helix_get_diagnostics` | `get-diagnostics` | LSP read (sync) |
| `helix_get_hover` | `get-hover-at` | LSP read (async) |
| `helix_get_definition` | `get-definition-at` | LSP read (async) |
| `helix_get_references` | `get-references-at` | LSP read (async) |
| `helix_get_workspace_symbols` | `get-workspace-symbols` | LSP read (async) |

What Phase 4b does NOT do:
- `format-document` and `helix_run_command` — Phase 6.
- `current-state`, `get-open-buffers`, `get-buffer-text` — these are already MCP Resources from Phase 4a (no point exposing them as Tools too).
- The `hook` subcommand — Phase 5.

## File structure

**Modify:**

- `helix-claude-mcp/src/serve.rs` — extend `HelixMcpServer` with `list_tools` and `call_tool` impls, plus a `dispatch_tool` helper that does the discovery + RPC roundtrip.
- `helix-claude-mcp/tests/integration.rs` — add integration tests that spawn the binary with a fake-Helix listener and exercise the tools end-to-end.

**Create:**

- `helix-claude-mcp/src/tools.rs` — `ToolKind` enum (parallel to `ResourceKind` in Phase 4a), per-tool input-schema definitions, the args structs.

**No new external dependencies.**

## Type design

`ToolKind` is the central enum, one variant per tool. It encapsulates name, description, and input schema:

```rust
pub enum ToolKind {
    HelixOpenFile,
    HelixGotoLine,
    HelixGetDiagnostics,
    HelixGetHover,
    HelixGetDefinition,
    HelixGetReferences,
    HelixGetWorkspaceSymbols,
}

impl ToolKind {
    pub const fn name(self) -> &'static str { ... }
    pub const fn description(self) -> &'static str { ... }
    pub fn input_schema(self) -> serde_json::Value { ... }   // JSON Schema
    pub fn from_name(name: &str) -> Option<Self> { ... }
    pub fn all() -> impl Iterator<Item = Self> { ... }
}
```

Per-tool args structs use `serde` derives:

```rust
#[derive(Deserialize)]
struct HelixOpenFileArgs { path: String }
#[derive(Deserialize)]
struct HelixGotoLineArgs { line: usize, column: Option<usize>, path: Option<String> }
// ...etc
```

Output: every tool returns its result as a JSON string in a single MCP `text` content. For writes, the string is `{"ok":true}`. For reads, the string is the relevant schema type serialized (the `Active` block for current-state-like content, etc.).

## Error mapping

Every tool error gets a clear text message:

- Discovery fail (no live socket) → `"Helix is not running in this workspace (no live control socket found). Start Helix with [editor.control-socket] enabled = true."`
- RPC connect/IO error → `"Failed to communicate with Helix: <details>"`
- Helix returned `JsonRpcError` → `"Helix rejected the request: <message> (code <code>)"`
- Malformed args → `"Invalid arguments: <serde error>"`

These surface to Claude as `is_error: true` MCP tool responses (rmcp's `CallToolResult::error` constructor).

---

## Task 1: Define `ToolKind` and centralize metadata

**Files:**
- Create: `helix-claude-mcp/src/tools.rs`
- Modify: `helix-claude-mcp/src/main.rs`

- [ ] **Step 1: Write failing tests**

Create `helix-claude-mcp/src/tools.rs`:

```rust
//! Tool definitions. One enum variant per MCP Tool exposed by helix-claude-mcp.
//! Each variant carries its public name, description, and JSON-Schema input
//! shape. The arg structs (HelixOpenFileArgs, etc.) live alongside for
//! ergonomic deserialization in serve::call_tool.

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    HelixOpenFile,
    HelixGotoLine,
    HelixGetDiagnostics,
    HelixGetHover,
    HelixGetDefinition,
    HelixGetReferences,
    HelixGetWorkspaceSymbols,
}

impl ToolKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::HelixOpenFile => "helix_open_file",
            Self::HelixGotoLine => "helix_goto_line",
            Self::HelixGetDiagnostics => "helix_get_diagnostics",
            Self::HelixGetHover => "helix_get_hover",
            Self::HelixGetDefinition => "helix_get_definition",
            Self::HelixGetReferences => "helix_get_references",
            Self::HelixGetWorkspaceSymbols => "helix_get_workspace_symbols",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::HelixOpenFile => {
                "Open a file in the running Helix editor and focus it. \
                 Path is absolute or relative to the workspace root."
            }
            Self::HelixGotoLine => {
                "Move the cursor in the running Helix editor to a 1-indexed line \
                 (and optional column). When path is given, switches to that buffer first; \
                 the buffer must already be open."
            }
            Self::HelixGetDiagnostics => {
                "Return all LSP diagnostics for a file (defaults to active buffer). \
                 Reads Helix's cached diagnostics; returns empty array if LSP is still \
                 analyzing or finds no issues."
            }
            Self::HelixGetHover => {
                "Return LSP hover info (type signature, doc) at a 1-indexed (line, column) \
                 position. Refuses with BufferModeUnsafe in Insert mode unless \
                 allow_insert_mode: true."
            }
            Self::HelixGetDefinition => {
                "Return LSP goto-definition locations for a symbol at a 1-indexed \
                 (line, column) position. Refuses in Insert mode unless allow_insert_mode: true."
            }
            Self::HelixGetReferences => {
                "Return all LSP references for a symbol at a 1-indexed (line, column) \
                 position. Set include_declaration: true to include the symbol's own \
                 declaration site (default: true). Refuses in Insert mode unless \
                 allow_insert_mode: true."
            }
            Self::HelixGetWorkspaceSymbols => {
                "Fuzzy-search workspace symbols by query string. Returns symbols with \
                 kind, name, location."
            }
        }
    }

    pub fn input_schema(self) -> Value {
        match self {
            Self::HelixOpenFile => json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path, absolute or workspace-relative" }
                },
                "required": ["path"]
            }),
            Self::HelixGotoLine => json!({
                "type": "object",
                "properties": {
                    "line": { "type": "integer", "minimum": 1, "description": "1-indexed line number" },
                    "column": { "type": "integer", "minimum": 1, "description": "1-indexed column number (optional; defaults to 1)" },
                    "path": { "type": "string", "description": "Buffer path (optional; defaults to active buffer). Must already be open." }
                },
                "required": ["line"]
            }),
            Self::HelixGetDiagnostics => json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" }
                }
            }),
            Self::HelixGetHover | Self::HelixGetDefinition => json!({
                "type": "object",
                "properties": {
                    "line": { "type": "integer", "minimum": 1 },
                    "column": { "type": "integer", "minimum": 1 },
                    "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" },
                    "allow_insert_mode": { "type": "boolean", "description": "Bypass the BufferModeUnsafe refusal" }
                },
                "required": ["line", "column"]
            }),
            Self::HelixGetReferences => json!({
                "type": "object",
                "properties": {
                    "line": { "type": "integer", "minimum": 1 },
                    "column": { "type": "integer", "minimum": 1 },
                    "path": { "type": "string" },
                    "allow_insert_mode": { "type": "boolean" },
                    "include_declaration": { "type": "boolean", "description": "Default: true" }
                },
                "required": ["line", "column"]
            }),
            Self::HelixGetWorkspaceSymbols => json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Fuzzy match query" }
                },
                "required": ["query"]
            }),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "helix_open_file" => Some(Self::HelixOpenFile),
            "helix_goto_line" => Some(Self::HelixGotoLine),
            "helix_get_diagnostics" => Some(Self::HelixGetDiagnostics),
            "helix_get_hover" => Some(Self::HelixGetHover),
            "helix_get_definition" => Some(Self::HelixGetDefinition),
            "helix_get_references" => Some(Self::HelixGetReferences),
            "helix_get_workspace_symbols" => Some(Self::HelixGetWorkspaceSymbols),
            _ => None,
        }
    }

    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::HelixOpenFile,
            Self::HelixGotoLine,
            Self::HelixGetDiagnostics,
            Self::HelixGetHover,
            Self::HelixGetDefinition,
            Self::HelixGetReferences,
            Self::HelixGetWorkspaceSymbols,
        ]
        .into_iter()
    }
}

#[derive(Debug, Deserialize)]
pub struct HelixOpenFileArgs {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct HelixGotoLineArgs {
    pub line: usize,
    #[serde(default)]
    pub column: Option<usize>,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HelixGetDiagnosticsArgs {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HelixPositionArgs {
    pub line: usize,
    pub column: usize,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub allow_insert_mode: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct HelixGetReferencesArgs {
    pub line: usize,
    pub column: usize,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub allow_insert_mode: Option<bool>,
    #[serde(default)]
    pub include_declaration: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct HelixGetWorkspaceSymbolsArgs {
    pub query: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_iterates_seven_kinds() {
        let kinds: Vec<_> = ToolKind::all().collect();
        assert_eq!(kinds.len(), 7);
    }

    #[test]
    fn names_are_underscored_not_dotted() {
        for kind in ToolKind::all() {
            assert!(
                !kind.name().contains('.'),
                "tool name {} has a dot; Claude Code's validator rejects dots",
                kind.name()
            );
            assert!(kind.name().starts_with("helix_"));
        }
    }

    #[test]
    fn from_name_round_trips_with_name() {
        for kind in ToolKind::all() {
            assert_eq!(ToolKind::from_name(kind.name()), Some(kind));
        }
    }

    #[test]
    fn from_name_unknown_returns_none() {
        assert_eq!(ToolKind::from_name("unknown_tool"), None);
    }

    #[test]
    fn input_schema_is_an_object_schema() {
        for kind in ToolKind::all() {
            let schema = kind.input_schema();
            assert_eq!(schema["type"], "object");
            assert!(schema["properties"].is_object());
        }
    }

    #[test]
    fn open_file_args_deserialize_minimal() {
        let v = json!({"path": "src/main.rs"});
        let a: HelixOpenFileArgs = serde_json::from_value(v).unwrap();
        assert_eq!(a.path, "src/main.rs");
    }

    #[test]
    fn goto_line_args_with_optionals_omitted() {
        let v = json!({"line": 42});
        let a: HelixGotoLineArgs = serde_json::from_value(v).unwrap();
        assert_eq!(a.line, 42);
        assert!(a.column.is_none());
        assert!(a.path.is_none());
    }

    #[test]
    fn position_args_deserialize_with_allow_insert() {
        let v = json!({"line": 5, "column": 10, "allow_insert_mode": true});
        let a: HelixPositionArgs = serde_json::from_value(v).unwrap();
        assert_eq!(a.line, 5);
        assert_eq!(a.column, 10);
        assert_eq!(a.allow_insert_mode, Some(true));
    }
}
```

- [ ] **Step 2: Wire into main.rs**

Edit `helix-claude-mcp/src/main.rs`. Add:

```rust
mod tools;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p helix-claude-mcp`
Expected: 16 prior + 8 new = 24 tests pass.

- [ ] **Step 4: Commit**

```bash
git add helix-claude-mcp/src/tools.rs helix-claude-mcp/src/main.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): ToolKind enum and arg structs for all 7 MCP tools

Centralizes per-tool metadata (name, description, JSON-Schema input
shape) and per-tool args structs (Deserialize-able from arbitrary JSON
that Claude sends in tools/call).

Tool names use underscores: helix_open_file, helix_goto_line,
helix_get_diagnostics, helix_get_hover, helix_get_definition,
helix_get_references, helix_get_workspace_symbols. The
names_are_underscored_not_dotted test guards against accidental dots
that Claude Code's validator rejects.

Eight tests cover enum iteration, name uniqueness, name round-trip,
JSON-Schema shape, and selective args deserialization.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.3)
EOF
)"
```

---

## Task 2: `dispatch_tool` helper + `list_tools` in `serve.rs`

**Files:**
- Modify: `helix-claude-mcp/src/serve.rs`

This task wires `list_tools` into the existing `ServerHandler` impl and adds a private helper `dispatch_tool` that:
1. Discovers the Helix socket via `discovery::find_helix_socket(None)`
2. Sends the constructed `ControlRequest` via `rpc_client::send_request`
3. Formats the response (or error) as a `CallToolResult`

Subsequent tasks (3-6) call `dispatch_tool` from per-tool match arms in `call_tool`.

- [ ] **Step 1: Read the existing `serve.rs`**

Run: `cat /Users/angm/helix/helix-claude-mcp/src/serve.rs`

Understand the existing `HelixMcpServer` struct and `ServerHandler` impl. Phase 4a set up `get_info`, `list_resources`, `read_resource`.

- [ ] **Step 2: Update `get_info` to advertise Tools capability**

Find the existing `get_info` body. The `ServerCapabilities` it returns should now include both `resources` and `tools`:

```rust
ServerCapabilities {
    resources: Some(ResourcesCapability::default()),
    tools: Some(ToolsCapability::default()),
    ..Default::default()
}
```

The exact rmcp type name may be `ToolsCapability` or `Tools` — check `cargo doc -p rmcp` or look at existing rmcp examples. If it's a different name, adjust.

- [ ] **Step 3: Add `list_tools` to `ServerHandler` impl**

```rust
    async fn list_tools(
        &self,
        _params: Option<PaginatedRequestParams>,
        _ctx: ...,    // rmcp's request context — check actual signature
    ) -> Result<ListToolsResult, ErrorData> {
        use crate::tools::ToolKind;
        let tools: Vec<Tool> = ToolKind::all()
            .map(|k| Tool {
                name: k.name().into(),
                description: Some(k.description().into()),
                input_schema: std::sync::Arc::new(
                    k.input_schema()
                        .as_object()
                        .expect("input_schema returns an object")
                        .clone(),
                ),
                annotations: None,
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }
```

The `Tool` struct shape and `ListToolsResult` constructor are rmcp 1.6.0-specific. Match what `list_resources` did in Phase 4a — `with_all_items` is the standard pattern.

The `input_schema` field type in rmcp may be `Arc<Map<String, Value>>` or `Arc<serde_json::Map<String, Value>>`. Adapt the closure accordingly.

- [ ] **Step 4: Add the `dispatch_tool` private helper**

Below the `ServerHandler` impl, add:

```rust
use anyhow::Context;
use helix_context_schema::{ControlRequest, ControlResponse, JsonRpcError};
use rmcp::model::{CallToolResult, Content};

/// Discover the Helix socket, send `request`, format the response as an
/// MCP tool result. Pure adapter between Phase 4a's plumbing and rmcp's
/// tool-result type.
async fn dispatch_tool(request: ControlRequest) -> CallToolResult {
    use crate::{discovery, rpc_client};

    let socket = match discovery::find_helix_socket(None).await {
        Ok(s) => s,
        Err(e) => return tool_error(format!(
            "Helix is not running in this workspace (no live control socket found): {}. \
             Start Helix with [editor.control-socket] enabled = true.",
            e,
        )),
    };

    match rpc_client::send_request(&socket, &request).await {
        Ok(resp) => format_response_as_tool_result(resp),
        Err(rpc_client::RpcError::HelixError(je)) => {
            tool_error(format!("Helix rejected the request: {} (code {})", je.message, je.code as i32))
        }
        Err(e) => {
            tool_error(format!("Failed to communicate with Helix: {}", e))
        }
    }
}

fn tool_error(message: String) -> CallToolResult {
    CallToolResult::error(vec![Content::text(message)])
}

fn format_response_as_tool_result(resp: ControlResponse) -> CallToolResult {
    let text = match resp {
        ControlResponse::Ok {} => "{\"ok\":true}".to_string(),
        other => serde_json::to_string(&other)
            .unwrap_or_else(|_| "{\"error\":\"serialization failed\"}".to_string()),
    };
    CallToolResult::success(vec![Content::text(text)])
}
```

The exact `CallToolResult::error` / `::success` constructor names may differ in rmcp 1.6.0. Check `cargo doc -p rmcp` for the available constructors. The principle: success path emits `text` content; error path returns the same content type but with `is_error: true`.

- [ ] **Step 5: Verify build**

Run: `cargo check -p helix-claude-mcp`
Expected: Clean. If `list_tools` has signature issues, look at how `list_resources` was implemented and follow the same pattern (same trait, same async signature shape).

- [ ] **Step 6: Update existing tests**

Run: `cargo test -p helix-claude-mcp`
Expected: 24 pass (8 new from Task 1 + 16 prior).

The integration test that asserts `"capabilities":{"resources":{}}` may need to be loosened to also expect `tools`. If a test was something like:

```rust
assert!(line.contains("\"capabilities\":{\"resources\":{}}"));
```

change to:

```rust
assert!(line.contains("\"resources\""));
assert!(line.contains("\"tools\""));
```

- [ ] **Step 7: Manual smoke test**

```bash
mkdir -p /tmp/p4b-t2/.helix
cat > /tmp/p4b-t2/.helix/context.json <<'EOF'
{"schema_version":2,"min_supported_reader":1,"timestamp":"2026-05-13T10:00:00Z","last_update_source":"focus_lost","project_root":"/tmp/p4b-t2","mode":"normal","active":{"path":null,"modified":false,"line_count":0,"cursors":[],"selections":[]},"open_buffers":[]}
EOF

cargo build --release -p helix-claude-mcp

CLAUDE_PROJECT_DIR=/tmp/p4b-t2 /Users/angm/helix/target/release/helix-claude-mcp serve <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0.1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
EOF

rm -rf /tmp/p4b-t2
```

Expected: `tools/list` response contains 7 tools, each with `name`, `description`, `inputSchema`.

- [ ] **Step 8: Commit**

```bash
git add helix-claude-mcp/src/serve.rs helix-claude-mcp/tests/integration.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): list_tools + dispatch_tool helper

ServerCapabilities now advertises tools alongside resources.
list_tools returns all 7 ToolKind variants with their input schemas.

dispatch_tool is the per-call adapter: discover socket → send_request →
format response. Used by call_tool arms in tasks 3-6. Errors at each
step surface as MCP tool errors (CallToolResult::error) with actionable
messages.

call_tool itself is still unimplemented — tasks 3-6 add the per-tool
arms that build ControlRequest and call dispatch_tool.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.3)
EOF
)"
```

---

## Task 3: `call_tool` dispatcher + `helix_open_file`

**Files:**
- Modify: `helix-claude-mcp/src/serve.rs`

- [ ] **Step 1: Add `call_tool` to `ServerHandler` impl**

In `serve.rs`, inside `impl ServerHandler for HelixMcpServer`, add (the exact signature shape comes from rmcp; consult `cargo doc -p rmcp`):

```rust
    async fn call_tool(
        &self,
        params: CallToolRequestParam,
        _ctx: ...,
    ) -> Result<CallToolResult, ErrorData> {
        use crate::tools::*;
        use helix_context_schema::ControlRequest;

        let name = params.name.as_ref();
        let args = params.arguments.unwrap_or(serde_json::Map::new());
        let args_val = serde_json::Value::Object(args);

        let kind = match ToolKind::from_name(name) {
            Some(k) => k,
            None => return Ok(tool_error(format!("Unknown tool: {}", name))),
        };

        let request = match kind {
            ToolKind::HelixOpenFile => {
                match serde_json::from_value::<HelixOpenFileArgs>(args_val) {
                    Ok(a) => ControlRequest::OpenFile { path: a.path },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_open_file: {}", e))),
                }
            }
            // The other six variants land in tasks 4-6 — bail for now.
            _ => return Ok(tool_error(format!(
                "Tool {} not yet implemented (Phase 4b in progress)",
                name
            ))),
        };

        Ok(dispatch_tool(request).await)
    }
```

The `params.name` field may be `Cow<'static, str>` or `Arc<str>` or `String` — check rmcp's type. `params.arguments` is likely `Option<serde_json::Map<String, Value>>`.

- [ ] **Step 2: Verify build**

Run: `cargo check -p helix-claude-mcp`
Expected: Clean.

- [ ] **Step 3: Run tests**

Run: `cargo test -p helix-claude-mcp`
Expected: 24 pass.

- [ ] **Step 4: Commit**

```bash
git add helix-claude-mcp/src/serve.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): call_tool dispatcher + helix_open_file arm

call_tool dispatches by ToolKind::from_name(...). Unknown names return
a clear MCP tool error. Each per-tool arm parses args (Deserialize) then
builds a ControlRequest, hands to dispatch_tool which does the socket
roundtrip and formats the response.

helix_open_file is the first arm — proves the pattern. Tasks 4-6 fill
in the remaining six. Until then, those names return a "not yet
implemented" tool error.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.3)
EOF
)"
```

---

## Task 4: `helix_goto_line` and `helix_get_diagnostics`

**Files:**
- Modify: `helix-claude-mcp/src/serve.rs`

- [ ] **Step 1: Add the two arms**

Find the `_ => return Ok(tool_error(...))` placeholder from Task 3. Add two new arms BEFORE the catchall:

```rust
            ToolKind::HelixGotoLine => {
                match serde_json::from_value::<HelixGotoLineArgs>(args_val) {
                    Ok(a) => ControlRequest::GotoLine {
                        line: a.line,
                        column: a.column,
                        path: a.path,
                    },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_goto_line: {}", e))),
                }
            }
            ToolKind::HelixGetDiagnostics => {
                match serde_json::from_value::<HelixGetDiagnosticsArgs>(args_val) {
                    Ok(a) => ControlRequest::GetDiagnostics { path: a.path },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_diagnostics: {}", e))),
                }
            }
```

- [ ] **Step 2: Verify build + tests**

Run: `cargo check --workspace && cargo test -p helix-claude-mcp`
Expected: 24 pass.

- [ ] **Step 3: Commit**

```bash
git add helix-claude-mcp/src/serve.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): helix_goto_line and helix_get_diagnostics arms

GotoLine takes line + optional column + optional path; defaults all
optionals to None. GetDiagnostics takes optional path (None = active
buffer).

Both follow the established Task 3 pattern: parse args, build
ControlRequest, hand to dispatch_tool.
EOF
)"
```

---

## Task 5: `helix_get_hover` and `helix_get_definition`

**Files:**
- Modify: `helix-claude-mcp/src/serve.rs`

These two tools share the same input shape (`HelixPositionArgs` from Task 1). They map to `GetHoverAt` and `GetDefinitionAt` respectively.

- [ ] **Step 1: Add the two arms**

Add before the catchall:

```rust
            ToolKind::HelixGetHover => {
                match serde_json::from_value::<HelixPositionArgs>(args_val) {
                    Ok(a) => ControlRequest::GetHoverAt {
                        line: a.line,
                        column: a.column,
                        path: a.path,
                        allow_insert_mode: a.allow_insert_mode,
                    },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_hover: {}", e))),
                }
            }
            ToolKind::HelixGetDefinition => {
                match serde_json::from_value::<HelixPositionArgs>(args_val) {
                    Ok(a) => ControlRequest::GetDefinitionAt {
                        line: a.line,
                        column: a.column,
                        path: a.path,
                        allow_insert_mode: a.allow_insert_mode,
                    },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_definition: {}", e))),
                }
            }
```

- [ ] **Step 2: Verify + test**

Run: `cargo check --workspace && cargo test -p helix-claude-mcp`
Expected: 24 pass.

- [ ] **Step 3: Commit**

```bash
git add helix-claude-mcp/src/serve.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): helix_get_hover and helix_get_definition arms

Both share HelixPositionArgs (line, column, optional path, optional
allow_insert_mode). They build GetHoverAt and GetDefinitionAt
ControlRequests respectively.

Helix's mode-aware refusal kicks in when allow_insert_mode is absent
or false and the editor is in Insert mode — surfaces as BufferModeUnsafe
(-32003) error that dispatch_tool maps to a friendly "Helix rejected"
MCP tool error.
EOF
)"
```

---

## Task 6: `helix_get_references` and `helix_get_workspace_symbols`

**Files:**
- Modify: `helix-claude-mcp/src/serve.rs`

- [ ] **Step 1: Add the two arms**

Add before the catchall:

```rust
            ToolKind::HelixGetReferences => {
                match serde_json::from_value::<HelixGetReferencesArgs>(args_val) {
                    Ok(a) => ControlRequest::GetReferencesAt {
                        line: a.line,
                        column: a.column,
                        path: a.path,
                        allow_insert_mode: a.allow_insert_mode,
                        include_declaration: a.include_declaration,
                    },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_references: {}", e))),
                }
            }
            ToolKind::HelixGetWorkspaceSymbols => {
                match serde_json::from_value::<HelixGetWorkspaceSymbolsArgs>(args_val) {
                    Ok(a) => ControlRequest::GetWorkspaceSymbols { query: a.query },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_workspace_symbols: {}", e))),
                }
            }
```

Now remove the `_ => return Ok(tool_error("not yet implemented"))` catchall — all seven variants are covered. The match becomes exhaustive over `ToolKind`.

- [ ] **Step 2: Verify + test**

Run: `cargo check --workspace && cargo test -p helix-claude-mcp`
Expected: 24 pass.

- [ ] **Step 3: Commit**

```bash
git add helix-claude-mcp/src/serve.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): helix_get_references and helix_get_workspace_symbols arms

GetReferences mirrors GetHover/Definition shape but adds the optional
include_declaration parameter (Helix defaults to true). GetWorkspaceSymbols
takes just a query string.

call_tool's match over ToolKind is now exhaustive — the "not yet
implemented" catchall is removed. All 7 tools are wired.

Phase 4b's primary deliverable is complete. Tasks 7-8 add integration
tests and update the README.
EOF
)"
```

---

## Task 7: Integration tests against a fake Helix listener

**Files:**
- Modify: `helix-claude-mcp/tests/integration.rs`

- [ ] **Step 1: Add a fake-Helix helper**

At the top of `integration.rs` (after the existing imports/constants), add:

```rust
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// Bind a fake Helix-side at `<workspace>/.helix/control-12345.sock` that
/// accepts one connection, reads one line of JSON-RPC request, sends back
/// `canned_response_line`, then drops. Returns the listener-bound socket
/// path so the test can confirm discovery picked it up.
async fn spawn_fake_helix_in(
    workspace: &std::path::Path,
    canned_response_line: String,
) -> std::path::PathBuf {
    let helix_dir = workspace.join(".helix");
    std::fs::create_dir_all(&helix_dir).unwrap();
    let sock = helix_dir.join("control-12345.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            // Drain the request line — content doesn't matter for the fake.
            use tokio::io::AsyncReadExt;
            let mut buf = vec![0u8; 8192];
            let _ = stream.read(&mut buf).await;
            let _ = stream.write_all(canned_response_line.as_bytes()).await;
            let _ = stream.flush().await;
        }
    });
    sock
}
```

- [ ] **Step 2: Add a tools/list integration test**

Append:

```rust
#[tokio::test]
async fn tools_list_returns_seven_tools() {
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
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..6 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                for tool in [
                    "helix_open_file", "helix_goto_line", "helix_get_diagnostics",
                    "helix_get_hover", "helix_get_definition", "helix_get_references",
                    "helix_get_workspace_symbols",
                ] {
                    assert!(line.contains(tool), "missing tool: {}\nfull line: {}", tool, line);
                }
                found = true;
                break;
            }
        }
    }
    assert!(found, "didn't see tools/list response");
    drop(stdin);
    let _ = child.kill().await;
}
```

- [ ] **Step 3: Add a tools/call test with the fake Helix**

```rust
#[tokio::test]
async fn tools_call_open_file_succeeds_against_fake_helix() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    // Fake Helix that responds to OpenFile with ControlResponse::Ok
    let canned = r#"{"method":"ok","result":{}}"#.to_string() + "\n";
    let _sock = spawn_fake_helix_in(tmp.path(), canned).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

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
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"helix_open_file","arguments":{"path":"src/main.rs"}}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..6 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                // Success response. Should contain the ok JSON in tool result content.
                assert!(line.contains("\"ok\"") || line.contains("ok"), "expected ok in response: {}", line);
                assert!(!line.contains("\"isError\":true"), "got error: {}", line);
                found = true;
                break;
            }
        }
    }
    assert!(found, "didn't see tools/call response");
    drop(stdin);
    let _ = child.kill().await;
}
```

- [ ] **Step 4: Add a tool error test (no Helix running)**

```rust
#[tokio::test]
async fn tools_call_returns_error_when_helix_not_running() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();
    // NO socket bound — discovery will fail.

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
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"helix_open_file","arguments":{"path":"x"}}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..6 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                // The tool call should return is_error: true with a message
                // about Helix not running.
                assert!(
                    line.contains("Helix is not running") || line.contains("not running"),
                    "expected friendly Helix-not-running message: {}", line
                );
                found = true;
                break;
            }
        }
    }
    assert!(found, "didn't see tools/call error response");
    drop(stdin);
    let _ = child.kill().await;
}
```

- [ ] **Step 5: Run integration tests**

Run: `cargo test -p helix-claude-mcp --test integration`
Expected: 3 prior + 3 new = 6 tests pass.

- [ ] **Step 6: Commit**

```bash
git add helix-claude-mcp/tests/integration.rs
git commit -m "$(cat <<'EOF'
test(claude-mcp): integration tests for tools/list and tools/call

Three new tests:
1. tools/list returns all 7 tool names — proves Task 1+2 wiring.
2. tools/call helix_open_file against a fake Helix listener — proves
   the full pipeline (call → discovery → RPC → response → MCP result).
3. tools/call without a running Helix — proves the friendly error
   message reaches the client.

The fake-Helix helper binds a tiny UnixListener, accepts one connection,
sends a canned response line, drops. No real Helix needed.
EOF
)"
```

---

## Task 8: Update README and final smoke test

**Files:**
- Modify: `helix-claude-mcp/README.md`

- [ ] **Step 1: Update the README**

Edit `helix-claude-mcp/README.md`. Find the section that mentions Tools as a future feature. Update to list all 7 tools as currently available. Append (or replace existing placeholder):

```markdown
## Available Tools

Phase 4b shipped these tools. Claude Code can call any of them via MCP `tools/call`:

| Tool | What it does |
|---|---|
| `helix_open_file` | Open a file in Helix and focus it. |
| `helix_goto_line` | Move the cursor to a line/column. |
| `helix_get_diagnostics` | List LSP diagnostics for a file. |
| `helix_get_hover` | LSP hover info at a position. |
| `helix_get_definition` | LSP goto-definition. |
| `helix_get_references` | LSP find-references. |
| `helix_get_workspace_symbols` | LSP workspace symbol search. |

All tools require Helix to be running with `[editor.control-socket] enabled = true`. When Helix isn't running, tools return a clear "not running" error message.
```

- [ ] **Step 2: Build a fresh release binary**

Run: `cargo build --release -p helix-claude-mcp`
Expected: Succeeds.

- [ ] **Step 3: Final end-to-end smoke against a real Helix**

This is the meaningful smoke test for Phase 4b — Claude (or any MCP client) drives the bridge, which drives a real Helix.

Setup:
```bash
mkdir -p /tmp/p4b-final && cd /tmp/p4b-final && git init -q
cat > Cargo.toml <<'EOF'
[package]
name = "p4b-final"
version = "0.1.0"
edition = "2021"
[[bin]]
name = "main"
path = "main.rs"
EOF
cat > main.rs <<'EOF'
fn helper() -> u32 { 42 }
fn main() {
    let x = helper();
    println!("{}", x);
}
EOF
```

In your global `~/.config/helix/config.toml`, ensure both `[editor.control-socket]` and `[editor.context-logger]` are `enabled = true`. (Back up the file first if you need to.)

Start Helix in a real terminal (the `script` PTY wrapper or just another terminal pane):
```bash
script -q /dev/null /Users/angm/helix/target/release/hx /tmp/p4b-final/main.rs &
HX_PID=$!
sleep 6  # rust-analyzer cold start
```

Drive helix-claude-mcp via stdin and verify it talks to the real Helix:
```bash
CLAUDE_PROJECT_DIR=/tmp/p4b-final /Users/angm/helix/target/release/helix-claude-mcp serve <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"final","version":"0.1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"helix_get_diagnostics","arguments":{}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"helix_get_workspace_symbols","arguments":{"query":"helper"}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"helix_get_hover","arguments":{"line":3,"column":13}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"helix_goto_line","arguments":{"line":1,"column":1}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"helix_open_file","arguments":{"path":"main.rs"}}}
EOF

pkill -P $HX_PID; kill $HX_PID
rm -rf /tmp/p4b-final
```

For each tool call, verify the response makes sense:
- `helix_get_diagnostics` — empty list (clean code) or LSP-style diagnostics
- `helix_get_workspace_symbols query=helper` — locations for the `helper` function
- `helix_get_hover line:3 col:13` — hover info for `helper`
- `helix_goto_line line:1` — `{"ok":true}` content
- `helix_open_file path:main.rs` — `{"ok":true}` content

If the test runs cleanly, Phase 4b is real. If LSP cold-start is slow, increase the sleep. If PTY allocation fails in the implementer's sandbox, skip — unit tests + integration tests cover the wire correctness.

- [ ] **Step 4: Commit**

```bash
git add helix-claude-mcp/README.md
git commit -m "$(cat <<'EOF'
docs(claude-mcp): README lists Phase 4b tools

Replaces the Phase 4a placeholder ("Tools will use socket in Phase 4b")
with the actual table of 7 available tools.

Phase 4b is complete. Claude Code can now drive Helix end-to-end:
open files, move cursor, query LSP for hover/definition/references/
diagnostics/workspace symbols.

Phase 5 adds the hook subcommand (replaces the shell hook). Phase 6
adds format-document, run-typable-command, doctor, telemetry.
EOF
)"
```

---

## Self-review checklist

After all 8 tasks:

- [ ] `cargo check --workspace` clean
- [ ] `cargo test -p helix-claude-mcp` — 24 unit + 6 integration = 30 tests pass
- [ ] `cargo test -p helix-context-schema` — 44 still pass (no schema changes)
- [ ] `cargo test -p helix-term control_socket` — 15 still pass (no helix-term changes)
- [ ] `cargo build --release -p helix-claude-mcp` succeeds
- [ ] Final end-to-end smoke (Task 8 Step 3) ran with a real Helix and rust-analyzer — at least one LSP method returned a useful payload, at least one write tool returned `{"ok":true}`, the discovery and roundtrip pipeline is verified live
- [ ] `git log --oneline -10` shows the Phase 4b commits in clean sequence

## What's NOT in Phase 4b

- `format-document` and `helix_run_command` — Phase 6.
- The `hook` subcommand — Phase 5.
- Connection pooling — single-request connections are fine for expected workload; revisit if perf data shows otherwise.

## Open questions

1. **rmcp's `Tool` and `CallToolResult` field shapes.** The plan's code shows the structure conceptually; implementer adjusts based on `cargo doc -p rmcp` for rmcp 1.6.0. T4 of Phase 4a captured the relevant patterns (`Annotated { raw, annotations: None }`, `with_all_items`, etc.); follow the same conventions for Tools.

2. **Tool error semantics on the wire.** This plan uses `CallToolResult::error(...)` (rmcp's idiomatic helper) so that the response carries `is_error: true` while still emitting a `text` content body. If rmcp's API requires different constructors (e.g. `CallToolResult { is_error: Some(true), content: vec![Content::text(...)] }` as a struct literal), adapt — but make sure the wire format includes `is_error: true` so Claude knows to treat the message as failure.

3. **Discovery cache.** Phase 4b discovers per tool call. A 5-second LRU cache on the discovered socket path would amortize across rapid tool sequences but adds state to `HelixMcpServer`. Worth doing only if perf data calls for it; the spec design (§7.5) explicitly says lazy connection-per-call is the v1 model.
