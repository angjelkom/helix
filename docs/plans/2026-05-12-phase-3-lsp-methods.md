# Phase 3 — LSP-Backed Methods — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose Helix's LSP knowledge through the control socket. Add five methods — `get-diagnostics`, `get-hover-at`, `get-definition-at`, `get-references-at`, `get-workspace-symbols` — plus mode-aware refusal so clients don't get garbage results from mid-typing positions.

**Architecture:** The four LSP-future methods follow a consistent pattern: the synchronous `handle_control_request` extracts everything it needs (LSP client clone, position, document URL, encoding) into a `'static` future; then it spawns a detached tokio task that awaits the future with a 10-second timeout and replies via the originating `oneshot::Sender`. The editor loop is unblocked instantly — main-thread time per LSP request is microseconds for setup. `get-diagnostics` is synchronous because Helix caches diagnostics on `Editor::diagnostics`.

**Tech Stack:** Same as Phases 2a/2b/2c — Tokio, serde, serde_json. Touches `helix-lsp` and `helix-lsp-types` for client handles and protocol types but does not pull them into `helix-context-schema`.

**Spec:** `docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md` §6.2 (LSP-backed methods), §6.5 (mid-typing safety).

---

## Project context for the implementer

You are working in a Helix editor fork at `/Users/angm/helix` on branch `nightly`. Phases 1, 2a, 2b, 2c are complete (tip: `b866eba89`, 43 commits ahead of remote). The control socket handles `initialize`, `current-state`, `get-open-buffers`, `get-buffer-text`, `open-file`, `goto-line`. 42 tests pass.

Phase 3 adds:

- Five `ControlRequest` variants: `GetDiagnostics`, `GetHoverAt`, `GetDefinitionAt`, `GetReferencesAt`, `GetWorkspaceSymbols`.
- Matching `ControlResponse` variants carrying LSP-style data (locations, hover contents, diagnostics, symbols).
- Six new schema types: `LspPosition`, `LspRange`, `LspLocation`, `LspHover`, `LspDiagnostic`, `LspSymbolInfo` (in `helix-context-schema`, no `lsp-types` dep).
- A mode-aware refusal helper (`BufferModeUnsafe` error when in insert mode without `allow_insert_mode: true`).
- An LSP-future-spawn helper that captures the future, spawns the task, applies the timeout, and sends the response back.
- Updated `initialize` capabilities advertising all five new methods.

What Phase 3 does NOT do:
- `format-document` — Phase 6.
- `run-typable-command` — deferred.
- The external `helix-claude-mcp` bridge binary — Phase 4.
- The Rust `hook` subcommand — Phase 5.

## Type design

We do NOT depend on `helix-lsp-types` from `helix-context-schema` — that crate must stay tiny and dependency-free for the future MCP bridge. Instead we define simplified data types in `helix-context-schema` that the helix-side handlers convert into. The MCP bridge binary will consume these schema types directly.

### New schema types

```rust
/// 0-indexed line and column. Matches LSP's `Position` semantics — note the
/// difference from `helix_context_schema::Position` (which is 1-indexed
/// because it's user-facing). LSP-derived data uses this 0-indexed form.
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

pub struct LspLocation {
    pub path: String,           // absolute or workspace-relative
    pub path_abs: String,        // always absolute
    pub range: LspRange,
}

pub struct LspHover {
    /// Hover content as plain text (Markdown is rendered to text). LSP's
    /// `MarkedString` / `MarkupContent` variants all get flattened here.
    pub contents: String,
    pub range: Option<LspRange>,
}

pub struct LspDiagnostic {
    pub range: LspRange,
    pub severity: Option<String>,   // "error", "warning", "information", "hint"
    pub code: Option<String>,
    pub source: Option<String>,     // language server name
    pub message: String,
}

pub struct LspSymbolInfo {
    pub name: String,
    pub kind: String,               // "function", "class", "variable", etc.
    pub location: LspLocation,
    pub container_name: Option<String>,
}
```

### New `ControlRequest` variants

```rust
GetDiagnostics {
    path: Option<String>,           // None = current buffer
},
GetHoverAt {
    line: usize,                    // 1-indexed
    column: usize,                  // 1-indexed
    path: Option<String>,
    allow_insert_mode: Option<bool>,
},
GetDefinitionAt {
    line: usize,
    column: usize,
    path: Option<String>,
    allow_insert_mode: Option<bool>,
},
GetReferencesAt {
    line: usize,
    column: usize,
    path: Option<String>,
    allow_insert_mode: Option<bool>,
    include_declaration: Option<bool>,
},
GetWorkspaceSymbols {
    query: String,
},
```

### New `ControlResponse` variants

```rust
GetDiagnostics {
    diagnostics: Vec<LspDiagnostic>,
},
GetHoverAt {
    hover: Option<LspHover>,
},
GetDefinitionAt {
    locations: Vec<LspLocation>,
},
GetReferencesAt {
    locations: Vec<LspLocation>,
},
GetWorkspaceSymbols {
    symbols: Vec<LspSymbolInfo>,
},
```

## File structure

**Modify:**

- `helix-context-schema/src/protocol.rs` — six new helper types + five request/response variants.
- `helix-context-schema/src/lib.rs` — re-export new types.
- `helix-context-schema/tests/protocol_roundtrip.rs` — round-trip tests for new types and variants.
- `helix-term/src/control_socket/dispatch.rs` — route new variants to `None`; advertise in capabilities.
- `helix-term/src/application.rs` — five new handler arms in `handle_control_request`. Mode-aware refusal helper. LSP-future-spawn helper.

**No new files.**

---

## Task 1: Add LSP-shaped types to `helix-context-schema`

**Files:**
- Modify: `helix-context-schema/src/protocol.rs`
- Modify: `helix-context-schema/src/lib.rs`
- Modify: `helix-context-schema/tests/protocol_roundtrip.rs`

- [ ] **Step 1: Write failing tests**

Append to `helix-context-schema/tests/protocol_roundtrip.rs`:

```rust
use helix_context_schema::{
    LspDiagnostic, LspHover, LspLocation, LspPosition, LspRange, LspSymbolInfo,
};

#[test]
fn lsp_position_serializes_zero_indexed() {
    let p = LspPosition { line: 0, character: 0 };
    let j = serde_json::to_value(&p).unwrap();
    assert_eq!(j["line"], 0);
    assert_eq!(j["character"], 0);
}

#[test]
fn lsp_range_round_trips() {
    let r = LspRange {
        start: LspPosition { line: 0, character: 5 },
        end: LspPosition { line: 2, character: 10 },
    };
    let j = serde_json::to_value(&r).unwrap();
    let back: LspRange = serde_json::from_value(j).unwrap();
    assert_eq!(back.start.line, 0);
    assert_eq!(back.end.character, 10);
}

#[test]
fn lsp_location_round_trips() {
    let loc = LspLocation {
        path: "src/main.rs".into(),
        path_abs: "/repo/src/main.rs".into(),
        range: LspRange {
            start: LspPosition { line: 0, character: 0 },
            end: LspPosition { line: 0, character: 5 },
        },
    };
    let j = serde_json::to_value(&loc).unwrap();
    assert_eq!(j["path"], "src/main.rs");
    let back: LspLocation = serde_json::from_value(j).unwrap();
    assert_eq!(back.path_abs, "/repo/src/main.rs");
}

#[test]
fn lsp_hover_omits_optional_range() {
    let h = LspHover { contents: "fn foo()".into(), range: None };
    let j = serde_json::to_value(&h).unwrap();
    assert!(j.get("range").is_none() || j["range"].is_null());
    assert_eq!(j["contents"], "fn foo()");
}

#[test]
fn lsp_diagnostic_serializes_with_all_fields() {
    let d = LspDiagnostic {
        range: LspRange {
            start: LspPosition { line: 5, character: 10 },
            end: LspPosition { line: 5, character: 15 },
        },
        severity: Some("error".into()),
        code: Some("E0308".into()),
        source: Some("rustc".into()),
        message: "expected `u32`, found `String`".into(),
    };
    let j = serde_json::to_value(&d).unwrap();
    assert_eq!(j["severity"], "error");
    assert_eq!(j["code"], "E0308");
    assert_eq!(j["source"], "rustc");
    assert_eq!(j["message"], "expected `u32`, found `String`");
    let back: LspDiagnostic = serde_json::from_value(j).unwrap();
    assert_eq!(back.range.start.line, 5);
}

#[test]
fn lsp_symbol_info_round_trips() {
    let s = LspSymbolInfo {
        name: "main".into(),
        kind: "function".into(),
        location: LspLocation {
            path: "src/main.rs".into(),
            path_abs: "/repo/src/main.rs".into(),
            range: LspRange {
                start: LspPosition { line: 0, character: 0 },
                end: LspPosition { line: 4, character: 1 },
            },
        },
        container_name: None,
    };
    let j = serde_json::to_value(&s).unwrap();
    assert_eq!(j["name"], "main");
    assert!(j.get("container_name").is_none() || j["container_name"].is_null());
    let back: LspSymbolInfo = serde_json::from_value(j).unwrap();
    assert_eq!(back.kind, "function");
}
```

- [ ] **Step 2: Run, confirm failure**

Run: `cargo test -p helix-context-schema`
Expected: 6 new tests fail — types don't exist.

- [ ] **Step 3: Add the types**

Append to `helix-context-schema/src/protocol.rs` (anywhere before the `ControlRequest` enum):

```rust
/// 0-indexed position. Matches LSP's `Position` semantics. Distinct from
/// `helix_context_schema::Position` (1-indexed, user-facing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspLocation {
    /// Workspace-relative path when possible, otherwise absolute.
    pub path: String,
    /// Always-absolute path. Lets clients disambiguate.
    pub path_abs: String,
    pub range: LspRange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspHover {
    /// Hover content flattened to plain text. LSP's `MarkupContent`
    /// variants (Markdown, plaintext) are all serialized to a single string.
    pub contents: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub range: Option<LspRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnostic {
    pub range: LspRange,
    /// "error" | "warning" | "information" | "hint". String to avoid
    /// pulling in LSP enums; consumers can compare to the four known values.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSymbolInfo {
    pub name: String,
    /// Symbol kind as a lowercase string ("function", "class", "variable").
    pub kind: String,
    pub location: LspLocation,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub container_name: Option<String>,
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

Edit `helix-context-schema/src/lib.rs`. Extend the `pub use protocol::{...}` line to include all six new types:

```rust
pub use protocol::{
    ClientInfo, ControlRequest, ControlResponse, LineRange, LspDiagnostic, LspHover,
    LspLocation, LspPosition, LspRange, LspSymbolInfo, ServerCapabilities, ServerInfo,
};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p helix-context-schema`
Expected: 34 tests pass (28 prior + 6 new).

- [ ] **Step 6: Commit**

```bash
git add helix-context-schema/src/protocol.rs helix-context-schema/src/lib.rs helix-context-schema/tests/protocol_roundtrip.rs
git commit -m "$(cat <<'EOF'
feat(context-schema): add LSP-shaped helper types

LspPosition (0-indexed, matches LSP semantics), LspRange, LspLocation
(with both path and path_abs for client convenience), LspHover (contents
flattened to plain text), LspDiagnostic (severity as string), LspSymbolInfo.

These types live in helix-context-schema with no lsp-types dep, so the
future MCP bridge binary can consume them without dragging the LSP
protocol stack along.

Six new round-trip tests cover wire format and optional-field omission.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.2)
EOF
)"
```

---

## Task 2: Add five new `ControlRequest`/`ControlResponse` variants

**Files:**
- Modify: `helix-context-schema/src/protocol.rs`
- Modify: `helix-context-schema/tests/protocol_roundtrip.rs`
- Modify: `helix-term/src/control_socket/dispatch.rs`
- Modify: `helix-term/src/application.rs`

- [ ] **Step 1: Write failing tests**

Append to `helix-context-schema/tests/protocol_roundtrip.rs`:

```rust
#[test]
fn get_diagnostics_request_with_no_path() {
    let req = ControlRequest::GetDiagnostics { path: None };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-diagnostics");
    assert!(j["params"].get("path").is_none() || j["params"]["path"].is_null());
}

#[test]
fn get_hover_at_request_round_trips() {
    let req = ControlRequest::GetHoverAt {
        line: 10,
        column: 5,
        path: Some("src/main.rs".into()),
        allow_insert_mode: Some(false),
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-hover-at");
    assert_eq!(j["params"]["line"], 10);
    let back: ControlRequest = serde_json::from_value(j).unwrap();
    let ControlRequest::GetHoverAt { line, column, .. } = back else {
        panic!("wrong variant");
    };
    assert_eq!(line, 10);
    assert_eq!(column, 5);
}

#[test]
fn get_definition_at_request_omits_optional_fields() {
    let req = ControlRequest::GetDefinitionAt {
        line: 1,
        column: 1,
        path: None,
        allow_insert_mode: None,
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-definition-at");
    assert!(j["params"].get("path").is_none() || j["params"]["path"].is_null());
    assert!(
        j["params"].get("allow_insert_mode").is_none()
            || j["params"]["allow_insert_mode"].is_null()
    );
}

#[test]
fn get_references_at_request_with_include_declaration() {
    let req = ControlRequest::GetReferencesAt {
        line: 5,
        column: 3,
        path: None,
        allow_insert_mode: None,
        include_declaration: Some(true),
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["params"]["include_declaration"], true);
}

#[test]
fn get_workspace_symbols_request() {
    let req = ControlRequest::GetWorkspaceSymbols { query: "main".into() };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-workspace-symbols");
    assert_eq!(j["params"]["query"], "main");
}

#[test]
fn hover_response_with_some_hover() {
    let resp = ControlResponse::GetHoverAt {
        hover: Some(LspHover {
            contents: "fn main()".into(),
            range: None,
        }),
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "get-hover-at");
    assert_eq!(j["result"]["hover"]["contents"], "fn main()");
}

#[test]
fn hover_response_with_none() {
    let resp = ControlResponse::GetHoverAt { hover: None };
    let j = serde_json::to_value(&resp).unwrap();
    assert!(j["result"]["hover"].is_null());
}

#[test]
fn definition_response_with_locations() {
    let resp = ControlResponse::GetDefinitionAt {
        locations: vec![LspLocation {
            path: "src/lib.rs".into(),
            path_abs: "/repo/src/lib.rs".into(),
            range: LspRange {
                start: LspPosition { line: 10, character: 0 },
                end: LspPosition { line: 12, character: 1 },
            },
        }],
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["result"]["locations"][0]["path"], "src/lib.rs");
}

#[test]
fn diagnostics_response_with_empty_list() {
    let resp = ControlResponse::GetDiagnostics { diagnostics: vec![] };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "get-diagnostics");
    assert_eq!(j["result"]["diagnostics"], serde_json::json!([]));
}

#[test]
fn workspace_symbols_response_round_trips() {
    let resp = ControlResponse::GetWorkspaceSymbols {
        symbols: vec![LspSymbolInfo {
            name: "main".into(),
            kind: "function".into(),
            location: LspLocation {
                path: "src/main.rs".into(),
                path_abs: "/repo/src/main.rs".into(),
                range: LspRange {
                    start: LspPosition { line: 0, character: 3 },
                    end: LspPosition { line: 0, character: 7 },
                },
            },
            container_name: None,
        }],
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["result"]["symbols"][0]["kind"], "function");
}
```

- [ ] **Step 2: Run failing**

Run: `cargo test -p helix-context-schema`
Expected: 10 new tests fail — variants don't exist.

- [ ] **Step 3: Add the variants**

Edit `helix-context-schema/src/protocol.rs`. Extend the `ControlRequest` enum (add after `GotoLine`):

```rust
    GetDiagnostics {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
    },
    GetHoverAt {
        line: usize,
        column: usize,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        allow_insert_mode: Option<bool>,
    },
    GetDefinitionAt {
        line: usize,
        column: usize,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        allow_insert_mode: Option<bool>,
    },
    GetReferencesAt {
        line: usize,
        column: usize,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        allow_insert_mode: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        include_declaration: Option<bool>,
    },
    GetWorkspaceSymbols {
        query: String,
    },
```

Extend `ControlResponse` (after `Ok`):

```rust
    GetDiagnostics {
        diagnostics: Vec<LspDiagnostic>,
    },
    GetHoverAt {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        hover: Option<LspHover>,
    },
    GetDefinitionAt {
        locations: Vec<LspLocation>,
    },
    GetReferencesAt {
        locations: Vec<LspLocation>,
    },
    GetWorkspaceSymbols {
        symbols: Vec<LspSymbolInfo>,
    },
```

Note: `hover` is `Option` because the LSP response itself can be null (the server has no hover for that position). The `skip_serializing_if` causes it to be absent from the JSON when None, which the test asserts.

Wait — the test `hover_response_with_none` asserts `j["result"]["hover"].is_null()`. That means we want `null` on the wire, not omission. So drop the `skip_serializing_if` from `hover`:

```rust
    GetHoverAt {
        hover: Option<LspHover>,
    },
```

This will serialize `None` as `null` (default serde Option behavior). Match the test.

- [ ] **Step 4: Stub the new variants in `dispatch.rs`**

Edit `helix-term/src/control_socket/dispatch.rs`. Find `try_dispatch_inline` and extend the `|` chain:

```rust
        ControlRequest::CurrentState {}
        | ControlRequest::GetOpenBuffers {}
        | ControlRequest::GetBufferText { .. }
        | ControlRequest::OpenFile { .. }
        | ControlRequest::GotoLine { .. }
        | ControlRequest::GetDiagnostics { .. }
        | ControlRequest::GetHoverAt { .. }
        | ControlRequest::GetDefinitionAt { .. }
        | ControlRequest::GetReferencesAt { .. }
        | ControlRequest::GetWorkspaceSymbols { .. } => None,
```

- [ ] **Step 5: Stub the new arms in `handle_control_request`**

Edit `helix-term/src/application.rs`. Add five MethodNotFound stubs in `handle_control_request` for each new variant. Use a single pattern:

```rust
            ControlRequest::GetDiagnostics { .. } => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "get-diagnostics handler not yet implemented".into(),
                    data: None,
                })
            }
            ControlRequest::GetHoverAt { .. } => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "get-hover-at handler not yet implemented".into(),
                    data: None,
                })
            }
            ControlRequest::GetDefinitionAt { .. } => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "get-definition-at handler not yet implemented".into(),
                    data: None,
                })
            }
            ControlRequest::GetReferencesAt { .. } => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "get-references-at handler not yet implemented".into(),
                    data: None,
                })
            }
            ControlRequest::GetWorkspaceSymbols { .. } => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "get-workspace-symbols handler not yet implemented".into(),
                    data: None,
                })
            }
```

- [ ] **Step 6: Verify + test**

Run: `cargo check --workspace` — clean.
Run: `cargo test -p helix-context-schema` — 44 tests pass (34 + 10 new).
Run: `cargo test -p helix-term control_socket` — 14 pass.

- [ ] **Step 7: Commit**

```bash
git add helix-context-schema/src/protocol.rs helix-context-schema/tests/protocol_roundtrip.rs helix-term/src/control_socket/dispatch.rs helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(context-schema): add LSP-backed method variants

Five new ControlRequest variants: GetDiagnostics, GetHoverAt,
GetDefinitionAt, GetReferencesAt, GetWorkspaceSymbols. Matching response
variants with hover/locations/diagnostics/symbols payloads.

All LSP query methods (Hover/DefinitionAt/ReferencesAt) take an optional
`allow_insert_mode` flag. Default behavior (None or false): refuse with
BufferModeUnsafe (-32003) when editor is in Insert mode, since mid-typing
positions return garbage from LSP. Phase 3 task 3 implements the gate.

10 new round-trip tests cover wire format. Handlers are stubs returning
MethodNotFound — real implementations in tasks 4-8.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.2, §6.5)
EOF
)"
```

---

## Task 3: Mode-aware refusal helper

**Files:**
- Modify: `helix-term/src/application.rs`

A small helper that LSP-position methods call before doing any work. Returns `Err(BufferModeUnsafe)` if the editor is in `Mode::Insert` and the caller didn't explicitly opt in.

- [ ] **Step 1: Add the helper**

Edit `helix-term/src/application.rs`. Near `resolve_buffer` (the Phase 2b helper), add:

```rust
/// Returns Err(BufferModeUnsafe) if the editor is in Insert mode and the
/// caller didn't pass `allow_insert_mode: true`. Used by LSP-position
/// methods to avoid querying garbage mid-typing positions.
fn ensure_buffer_mode_safe(
    editor: &helix_view::Editor,
    allow_insert_mode: Option<bool>,
) -> Result<(), helix_context_schema::JsonRpcError> {
    use helix_context_schema::{JsonRpcError, JsonRpcErrorCode};
    if editor.mode == helix_view::document::Mode::Insert
        && !allow_insert_mode.unwrap_or(false)
    {
        return Err(JsonRpcError {
            code: JsonRpcErrorCode::BufferModeUnsafe,
            message: "editor is in insert mode; pass allow_insert_mode: true to override".into(),
            data: None,
        });
    }
    Ok(())
}
```

- [ ] **Step 2: Verify build**

Run: `cargo check --workspace`
Expected: Clean. (Helper is `dead_code` until tasks 5-7 use it — `cargo check` allows that; if it complains, add `#[allow(dead_code)]` until task 5.)

- [ ] **Step 3: Commit (small, plumbing only)**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): mode-aware refusal helper

ensure_buffer_mode_safe returns BufferModeUnsafe (-32003) when the
editor is in Insert mode and the caller didn't opt in via
allow_insert_mode. Used by upcoming LSP-position methods (hover,
definition, references) to avoid querying garbage cursor positions
mid-typing.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.5)
EOF
)"
```

---

## Task 4: Implement `get-diagnostics` handler

**Files:**
- Modify: `helix-term/src/application.rs`

Easiest of the five methods because Helix caches diagnostics on `Editor::diagnostics` — no LSP future needed. Sync handler. Inspecting that field, mapping each diagnostic to `LspDiagnostic`.

- [ ] **Step 1: Inspect `Editor::diagnostics`**

Run: `grep -n "pub diagnostics\|struct Diagnostics" /Users/angm/helix/helix-view/src/editor.rs | head -10`

You should find something like `pub diagnostics: Diagnostics` (or similar). Look at the `Diagnostics` struct to see how to iterate per-document. Typical shape:

```rust
pub struct Diagnostics {
    inner: HashMap<Url, Vec<DiagnosticEntry>>,
}
```

The iteration API is something like `editor.diagnostics.get(&url)` or `editor.diagnostics.iter()`. The exact API varies — read the source and adjust.

- [ ] **Step 2: Replace the `GetDiagnostics` stub**

Edit `helix-term/src/application.rs`. Find the `ControlRequest::GetDiagnostics { .. }` stub. Replace with (adapt the diagnostic-iteration code to whatever the actual API looks like):

```rust
            ControlRequest::GetDiagnostics { path } => {
                let (workspace, _) = helix_loader::find_workspace();
                let doc = match resolve_buffer(&self.editor, &workspace, path.as_deref()) {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = reply.send(Err(e));
                        return;
                    }
                };
                let Some(doc_url) = doc.url() else {
                    let _ = reply.send(Err(JsonRpcError {
                        code: JsonRpcErrorCode::NoActiveDocument,
                        message: "document has no URL (scratch buffer?)".into(),
                        data: None,
                    }));
                    return;
                };

                let mut diagnostics_out: Vec<helix_context_schema::LspDiagnostic> = Vec::new();
                if let Some(entries) = self.editor.diagnostics.get(&doc_url) {
                    for (diag, _provider) in entries.iter() {
                        diagnostics_out.push(helix_context_schema::LspDiagnostic {
                            range: helix_context_schema::LspRange {
                                start: helix_context_schema::LspPosition {
                                    line: diag.range.start.line,
                                    character: diag.range.start.character,
                                },
                                end: helix_context_schema::LspPosition {
                                    line: diag.range.end.line,
                                    character: diag.range.end.character,
                                },
                            },
                            severity: diag.severity.map(|s| match s {
                                helix_lsp_types::DiagnosticSeverity::ERROR => "error",
                                helix_lsp_types::DiagnosticSeverity::WARNING => "warning",
                                helix_lsp_types::DiagnosticSeverity::INFORMATION => "information",
                                helix_lsp_types::DiagnosticSeverity::HINT => "hint",
                                _ => "unknown",
                            }.into()),
                            code: diag.code.as_ref().map(|c| match c {
                                helix_lsp_types::NumberOrString::Number(n) => n.to_string(),
                                helix_lsp_types::NumberOrString::String(s) => s.clone(),
                            }),
                            source: diag.source.clone(),
                            message: diag.message.clone(),
                        });
                    }
                }

                let _ = reply.send(Ok(ControlResponse::GetDiagnostics {
                    diagnostics: diagnostics_out,
                }));
                return;
            }
```

This code uses `helix_lsp_types` for the severity enum and code variants. If those exact type names don't exist in this fork's branch, adjust. The goal: map Helix's internal diagnostic representation to the schema's `LspDiagnostic`.

If `editor.diagnostics.get(&doc_url)` returns a `Vec<(Diagnostic, ProviderId)>` (or similar tuple), unpack accordingly. The iteration pattern may need adjustment.

- [ ] **Step 3: Verify build**

Run: `cargo check --workspace`
Expected: May fail with type-name mismatches in the diagnostic mapping. Adjust based on actual API. The compiler errors will be specific — fix until clean.

- [ ] **Step 4: Run tests**

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 44 + 14 pass.

- [ ] **Step 5: Smoke test**

Standard pattern: `script -q /dev/null` to start hx, then a Python client. For diagnostics testing, create a file with intentional errors so the LSP populates diagnostics. Wait a beat for LSP to analyze before querying:

```bash
mkdir -p /tmp/p3-diag && cd /tmp/p3-diag && git init -q
# Create a file with a deliberate Rust error
echo 'fn main() { let x: u32 = "string"; }' > main.rs
# Ensure rust-analyzer can find it
cat > Cargo.toml <<'EOF'
[package]
name = "p3-diag-test"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "main"
path = "main.rs"
EOF
cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 5  # rust-analyzer takes time to start + analyze
SOCK=$(ls /tmp/p3-diag/.helix/control-*.sock | head -1)
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect('$SOCK')
s.sendall(b'{\"method\":\"get-diagnostics\",\"params\":{}}\n')
print(json.dumps(json.loads(s.recv(16384).decode()), indent=2))
"
pkill -P $HX_PID; kill $HX_PID; sleep 0.5
rm -rf /tmp/p3-diag
```

Expected: a `get-diagnostics` response with at least one `LspDiagnostic` showing the type mismatch error. If diagnostics are empty, LSP may not have finished analysis — increase the `sleep` or just confirm the empty list is what's returned and the server has no diagnostics.

- [ ] **Step 6: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement get-diagnostics method

Reads from editor.diagnostics (cached by Helix per-document). Maps
Helix's internal Diagnostic representation to the schema's LspDiagnostic
(severity as string, code as string regardless of LSP's number/string
union).

Sync handler — no LSP future. Returns empty list if the document has
no diagnostics yet (e.g. LSP still analyzing).

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.2)
EOF
)"
```

---

## Task 5: LSP future-spawn infrastructure

**Files:**
- Modify: `helix-term/src/application.rs`

The four LSP-future methods (hover, definition, references, workspace-symbols) all follow the same pattern: extract a `Send + 'static` future from the LSP client clone, spawn a tokio task, timeout the future, send the response back. Build this as a helper so each handler is short.

- [ ] **Step 1: Add the helper**

Add to `helix-term/src/application.rs` near `resolve_buffer`:

```rust
/// Spawn an LSP request future as a detached tokio task. The task awaits
/// the future with a 10-second timeout and sends the result back via the
/// originating oneshot. The editor event loop is unblocked instantly.
///
/// `convert` is called from the task to map the LSP response into a
/// ControlResponse. `convert` is `Send + 'static` and runs off the
/// editor thread; it should not touch &Editor or any of its derivatives.
fn spawn_lsp_request<F, T, C>(
    reply: tokio::sync::oneshot::Sender<
        Result<
            helix_context_schema::ControlResponse,
            helix_context_schema::JsonRpcError,
        >,
    >,
    future: F,
    convert: C,
) where
    F: std::future::Future<Output = Result<T, helix_lsp::jsonrpc::Error>> + Send + 'static,
    T: Send + 'static,
    C: FnOnce(T) -> helix_context_schema::ControlResponse + Send + 'static,
{
    use helix_context_schema::{JsonRpcError, JsonRpcErrorCode};
    tokio::spawn(async move {
        let resp = match tokio::time::timeout(std::time::Duration::from_secs(10), future).await {
            Ok(Ok(value)) => Ok(convert(value)),
            Ok(Err(e)) => Err(JsonRpcError {
                code: JsonRpcErrorCode::InternalError,
                message: format!("LSP error: {}", e),
                data: None,
            }),
            Err(_) => Err(JsonRpcError {
                code: JsonRpcErrorCode::LspTimeout,
                message: "LSP request timed out after 10s".into(),
                data: None,
            }),
        };
        let _ = reply.send(resp);
    });
}
```

The exact error type from Helix's LSP machinery is `helix_lsp::jsonrpc::Error` per the `helix-lsp` crate. If the actual path differs (e.g. `helix_lsp::Error` or `helix_lsp::client::Error`), adjust the bound.

If the LSP client's request methods return some other error type (or wrap the response in `Option`), adapt the helper signature. The shape `Future<Output = Result<T, _>>` is standard for LSP request futures, but the inner Result may differ.

- [ ] **Step 2: Verify build**

Run: `cargo check --workspace`
Expected: May complain about the LSP error type or the future signature. Adjust based on actual LSP client API. Add `#[allow(dead_code)]` on `spawn_lsp_request` for now since it has no callers yet (tasks 6-8 use it).

- [ ] **Step 3: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): spawn_lsp_request helper

Detached-task pattern for LSP request futures: the main event loop
extracts a Send + 'static future from the cloned LSP client handle,
calls spawn_lsp_request to hand it off, and is immediately free.

The task wraps the future in a 10-second timeout. On success it calls
the convert closure to map LSP response → ControlResponse; on LSP
protocol error returns InternalError; on timeout returns LspTimeout
(-32002).

No callers yet — tasks 6-8 use this for hover/definition/references.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§5.4)
EOF
)"
```

---

## Task 6: Implement `get-hover-at` handler

**Files:**
- Modify: `helix-term/src/application.rs`

First handler that uses the LSP-future-spawn pattern. The pattern proven here applies to definition and references identically.

- [ ] **Step 1: Inspect Helix's LSP client API for hover**

Run: `grep -n "text_document_hover\|fn hover" /Users/angm/helix/helix-lsp/src/client.rs | head -5`

Typical signature: `pub fn text_document_hover(&self, doc_id: TextDocumentIdentifier, pos: Position, work_done_token: Option<...>) -> impl Future<...>` — confirm the signature.

Also check how to convert Helix's 1-indexed `(line, column)` to LSP's `Position`:

Run: `grep -n "lsp_pos_to_pos\|pos_to_lsp_pos" /Users/angm/helix/helix-core/src/*.rs /Users/angm/helix/helix-lsp/src/*.rs | head -10`

There's a `helix_lsp::util::pos_to_lsp_pos` function. Use that to convert from char index to LSP position, with the LSP encoding from the server.

- [ ] **Step 2: Replace the `GetHoverAt` stub**

The general shape:

```rust
            ControlRequest::GetHoverAt { line, column, path, allow_insert_mode } => {
                if let Err(e) = ensure_buffer_mode_safe(&self.editor, allow_insert_mode) {
                    let _ = reply.send(Err(e));
                    return;
                }
                let (workspace, _) = helix_loader::find_workspace();
                let doc = match resolve_buffer(&self.editor, &workspace, path.as_deref()) {
                    Ok(d) => d,
                    Err(e) => { let _ = reply.send(Err(e)); return; }
                };
                let Some(doc_url) = doc.url() else {
                    let _ = reply.send(Err(JsonRpcError {
                        code: JsonRpcErrorCode::NoActiveDocument,
                        message: "document has no URL".into(),
                        data: None,
                    }));
                    return;
                };

                // Pick an LSP client supporting hover.
                let lsp_client = doc
                    .language_servers_with_feature(helix_view::editor::LanguageServerFeature::Hover)
                    .next();
                let Some(lsp_client) = lsp_client else {
                    let _ = reply.send(Err(JsonRpcError {
                        code: JsonRpcErrorCode::NoLspForLanguage,
                        message: "no LSP supporting hover for this document".into(),
                        data: None,
                    }));
                    return;
                };

                // Convert 1-indexed (line, column) to LSP Position.
                let text = doc.text();
                let target_line = line.saturating_sub(1).min(text.len_lines().saturating_sub(1));
                let target_col = column.saturating_sub(1);
                let line_start = text.line_to_char(target_line);
                let pos_char = line_start + target_col;
                let lsp_pos = helix_lsp::util::pos_to_lsp_pos(
                    text,
                    pos_char,
                    lsp_client.offset_encoding(),
                );

                let doc_url = doc_url.clone();
                let future = lsp_client.text_document_hover(doc_url, lsp_pos, None);

                spawn_lsp_request(reply, future, |hover_opt: Option<helix_lsp_types::Hover>| {
                    let hover = hover_opt.map(|h| helix_context_schema::LspHover {
                        contents: lsp_hover_contents_to_string(&h.contents),
                        range: h.range.map(|r| helix_context_schema::LspRange {
                            start: helix_context_schema::LspPosition {
                                line: r.start.line,
                                character: r.start.character,
                            },
                            end: helix_context_schema::LspPosition {
                                line: r.end.line,
                                character: r.end.character,
                            },
                        }),
                    });
                    helix_context_schema::ControlResponse::GetHoverAt { hover }
                });
                return;
            }
```

You'll also need a small helper to flatten LSP's `HoverContents` (Markdown / plaintext variants) to a single string. Add near `spawn_lsp_request`:

```rust
fn lsp_hover_contents_to_string(c: &helix_lsp_types::HoverContents) -> String {
    use helix_lsp_types::{HoverContents, MarkedString};
    match c {
        HoverContents::Scalar(MarkedString::String(s)) => s.clone(),
        HoverContents::Scalar(MarkedString::LanguageString(ls)) => {
            format!("```{}\n{}\n```", ls.language, ls.value)
        }
        HoverContents::Array(items) => items
            .iter()
            .map(|m| match m {
                MarkedString::String(s) => s.clone(),
                MarkedString::LanguageString(ls) => {
                    format!("```{}\n{}\n```", ls.language, ls.value)
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        HoverContents::Markup(mc) => mc.value.clone(),
    }
}
```

- [ ] **Step 3: Verify build**

Run: `cargo check --workspace`
Expected: Possibly more LSP type-name mismatches. The exact names of `LanguageServerFeature::Hover`, `pos_to_lsp_pos`, and the hover future's return type need to match the actual fork. Fix until clean.

The LSP client's `text_document_hover` returns `impl Future<Output = ...>`. The inner Output may be `Result<Option<Hover>, jsonrpc::Error>` or similar. Make `spawn_lsp_request` generic enough or adjust the call. The convert closure assumes `T = Option<Hover>`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 44 + 14 pass.

- [ ] **Step 5: Smoke test**

```bash
mkdir -p /tmp/p3-hov && cd /tmp/p3-hov && git init -q
cat > Cargo.toml <<'EOF'
[package]
name = "p3-hov-test"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "main"
path = "main.rs"
EOF
cat > main.rs <<'EOF'
fn main() {
    let x: u32 = 42;
    println!("{}", x);
}
EOF

cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 5  # rust-analyzer startup
SOCK=$(ls /tmp/p3-hov/.helix/control-*.sock | head -1)

# Hover on `x` at line 2 column 9
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect('$SOCK')
s.sendall(b'{\"method\":\"get-hover-at\",\"params\":{\"line\":2,\"column\":9}}\n')
print(json.dumps(json.loads(s.recv(8192).decode()), indent=2))
"

# Try in insert mode (should refuse)
# (Difficult to simulate from a stdio client; the unit test covers this)

pkill -P $HX_PID; kill $HX_PID; sleep 0.5
rm -rf /tmp/p3-hov
```

Expected: a `get-hover-at` response containing hover content for `x` (e.g. `"let x: u32"` or similar from rust-analyzer). May be empty if LSP not ready — increase `sleep` if needed.

- [ ] **Step 6: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement get-hover-at method

Mode-safe (refuses in Insert unless allow_insert_mode: true). Resolves
path → buffer, picks an LSP supporting Hover feature, builds the
text_document_hover future (Send + 'static), and hands off to
spawn_lsp_request — which awaits with 10s timeout off the main thread.

LSP HoverContents (MarkedString and MarkupContent variants) flattened to
a single plain-text string in schema's LspHover.contents.

First LSP-future method, proves the spawn pattern for definition/refs.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.2, §6.5)
EOF
)"
```

---

## Task 7: Implement `get-definition-at` handler

**Files:**
- Modify: `helix-term/src/application.rs`

Same pattern as hover. The LSP method is `text_document_definition` returning a `Send + 'static` future whose output is `Option<GotoDefinitionResponse>` (Helix's LSP type — a union of Location, Vec<Location>, or Vec<LocationLink>).

- [ ] **Step 1: Add an LSP-location → schema-location converter**

Near `lsp_hover_contents_to_string`, add:

```rust
fn lsp_locations_to_schema(
    locations: Vec<helix_lsp_types::Location>,
    workspace: &std::path::Path,
) -> Vec<helix_context_schema::LspLocation> {
    locations
        .into_iter()
        .filter_map(|loc| {
            let path_abs = loc.uri.to_file_path().ok()?;
            let path_rel = path_abs
                .strip_prefix(workspace)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path_abs.to_string_lossy().into_owned());
            Some(helix_context_schema::LspLocation {
                path: path_rel,
                path_abs: path_abs.to_string_lossy().into_owned(),
                range: helix_context_schema::LspRange {
                    start: helix_context_schema::LspPosition {
                        line: loc.range.start.line,
                        character: loc.range.start.character,
                    },
                    end: helix_context_schema::LspPosition {
                        line: loc.range.end.line,
                        character: loc.range.end.character,
                    },
                },
            })
        })
        .collect()
}
```

Also add a `GotoDefinitionResponse` → `Vec<Location>` flattener (LSP's definition response can be a single Location, a Vec, or a Vec<LocationLink>):

```rust
fn flatten_definition_response(
    resp: Option<helix_lsp_types::GotoDefinitionResponse>,
) -> Vec<helix_lsp_types::Location> {
    use helix_lsp_types::GotoDefinitionResponse;
    match resp {
        None => Vec::new(),
        Some(GotoDefinitionResponse::Scalar(loc)) => vec![loc],
        Some(GotoDefinitionResponse::Array(locs)) => locs,
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|l| helix_lsp_types::Location {
                uri: l.target_uri,
                range: l.target_range,
            })
            .collect(),
    }
}
```

- [ ] **Step 2: Replace the `GetDefinitionAt` stub**

```rust
            ControlRequest::GetDefinitionAt { line, column, path, allow_insert_mode } => {
                if let Err(e) = ensure_buffer_mode_safe(&self.editor, allow_insert_mode) {
                    let _ = reply.send(Err(e));
                    return;
                }
                let (workspace, _) = helix_loader::find_workspace();
                let doc = match resolve_buffer(&self.editor, &workspace, path.as_deref()) {
                    Ok(d) => d,
                    Err(e) => { let _ = reply.send(Err(e)); return; }
                };
                let Some(doc_url) = doc.url() else {
                    let _ = reply.send(Err(JsonRpcError {
                        code: JsonRpcErrorCode::NoActiveDocument,
                        message: "document has no URL".into(),
                        data: None,
                    }));
                    return;
                };
                let lsp_client = doc
                    .language_servers_with_feature(
                        helix_view::editor::LanguageServerFeature::GotoDefinition,
                    )
                    .next();
                let Some(lsp_client) = lsp_client else {
                    let _ = reply.send(Err(JsonRpcError {
                        code: JsonRpcErrorCode::NoLspForLanguage,
                        message: "no LSP supporting goto-definition for this document".into(),
                        data: None,
                    }));
                    return;
                };

                let text = doc.text();
                let target_line = line.saturating_sub(1).min(text.len_lines().saturating_sub(1));
                let target_col = column.saturating_sub(1);
                let pos_char = text.line_to_char(target_line) + target_col;
                let lsp_pos = helix_lsp::util::pos_to_lsp_pos(
                    text,
                    pos_char,
                    lsp_client.offset_encoding(),
                );

                let future = lsp_client.goto_definition(doc_url.clone(), lsp_pos, None);
                let workspace_clone = workspace.clone();

                spawn_lsp_request(reply, future, move |resp| {
                    let locations = flatten_definition_response(resp);
                    let schema_locs = lsp_locations_to_schema(locations, &workspace_clone);
                    helix_context_schema::ControlResponse::GetDefinitionAt {
                        locations: schema_locs,
                    }
                });
                return;
            }
```

The exact method name may be `goto_definition` or `text_document_definition` — confirm with `grep -n "fn goto_definition\|fn text_document_definition" /Users/angm/helix/helix-lsp/src/client.rs` and adjust.

- [ ] **Step 3: Verify + tests**

Run: `cargo check --workspace`
Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 44 + 14 pass.

- [ ] **Step 4: Smoke test**

```bash
mkdir -p /tmp/p3-def && cd /tmp/p3-def && git init -q
cat > Cargo.toml <<'EOF'
[package]
name = "p3-def-test"
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

cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 5
SOCK=$(ls /tmp/p3-def/.helix/control-*.sock | head -1)

# Definition of `helper` at line 3, column 13 (the helper() call)
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect('$SOCK')
s.sendall(b'{\"method\":\"get-definition-at\",\"params\":{\"line\":3,\"column\":13}}\n')
print(json.dumps(json.loads(s.recv(8192).decode()), indent=2))
"

pkill -P $HX_PID; kill $HX_PID; sleep 0.5
rm -rf /tmp/p3-def
```

Expected: locations array with one entry pointing to line 1 of `main.rs`.

- [ ] **Step 5: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement get-definition-at method

Same pattern as get-hover-at: mode-safe, resolves buffer, picks LSP
supporting GotoDefinition, spawns the future via spawn_lsp_request.

Flattens LSP's GotoDefinitionResponse (Scalar | Array | LocationLink)
to a uniform Vec<Location>, then maps each to schema's LspLocation
with both workspace-relative and absolute paths.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.2)
EOF
)"
```

---

## Task 8: Implement `get-references-at` and `get-workspace-symbols`

**Files:**
- Modify: `helix-term/src/application.rs`

Both are similar to definition. References uses `text_document_references` (or `goto_reference`); workspace symbols uses `workspace_symbol` (or `workspace_symbols`). Combining them in one task because the patterns are highly templated.

- [ ] **Step 1: Replace the `GetReferencesAt` stub**

```rust
            ControlRequest::GetReferencesAt {
                line, column, path, allow_insert_mode, include_declaration,
            } => {
                if let Err(e) = ensure_buffer_mode_safe(&self.editor, allow_insert_mode) {
                    let _ = reply.send(Err(e));
                    return;
                }
                let (workspace, _) = helix_loader::find_workspace();
                let doc = match resolve_buffer(&self.editor, &workspace, path.as_deref()) {
                    Ok(d) => d,
                    Err(e) => { let _ = reply.send(Err(e)); return; }
                };
                let Some(doc_url) = doc.url() else {
                    let _ = reply.send(Err(JsonRpcError {
                        code: JsonRpcErrorCode::NoActiveDocument,
                        message: "document has no URL".into(),
                        data: None,
                    }));
                    return;
                };
                let lsp_client = doc
                    .language_servers_with_feature(
                        helix_view::editor::LanguageServerFeature::GotoReference,
                    )
                    .next();
                let Some(lsp_client) = lsp_client else {
                    let _ = reply.send(Err(JsonRpcError {
                        code: JsonRpcErrorCode::NoLspForLanguage,
                        message: "no LSP supporting find-references for this document".into(),
                        data: None,
                    }));
                    return;
                };

                let text = doc.text();
                let target_line = line.saturating_sub(1).min(text.len_lines().saturating_sub(1));
                let target_col = column.saturating_sub(1);
                let pos_char = text.line_to_char(target_line) + target_col;
                let lsp_pos = helix_lsp::util::pos_to_lsp_pos(
                    text,
                    pos_char,
                    lsp_client.offset_encoding(),
                );

                let include = include_declaration.unwrap_or(true);
                let future = lsp_client.text_document_references(
                    doc_url.clone(),
                    lsp_pos,
                    include,
                    None,
                );
                let workspace_clone = workspace.clone();

                spawn_lsp_request(reply, future, move |resp: Option<Vec<helix_lsp_types::Location>>| {
                    let schema_locs = lsp_locations_to_schema(resp.unwrap_or_default(), &workspace_clone);
                    helix_context_schema::ControlResponse::GetReferencesAt {
                        locations: schema_locs,
                    }
                });
                return;
            }
```

Adjust `text_document_references`'s signature based on actual API. Helix's method may be named `goto_reference` and take `include_declaration` differently.

- [ ] **Step 2: Replace the `GetWorkspaceSymbols` stub**

Workspace symbols doesn't take a position — just a query string. Pick *any* LSP server that supports `WorkspaceSymbols` (Helix may iterate all servers and merge results; for simplicity we take the first).

```rust
            ControlRequest::GetWorkspaceSymbols { query } => {
                // Pick an LSP server with workspace-symbols feature from any open doc.
                // Iterate documents and find the first one with such a server.
                let lsp_client = self
                    .editor
                    .documents()
                    .filter_map(|d| {
                        d.language_servers_with_feature(
                            helix_view::editor::LanguageServerFeature::WorkspaceSymbols,
                        )
                        .next()
                    })
                    .next();
                let Some(lsp_client) = lsp_client else {
                    let _ = reply.send(Err(JsonRpcError {
                        code: JsonRpcErrorCode::NoLspForLanguage,
                        message: "no LSP supporting workspace-symbols".into(),
                        data: None,
                    }));
                    return;
                };

                let (workspace, _) = helix_loader::find_workspace();
                let workspace_clone = workspace.clone();
                let future = lsp_client.workspace_symbols(query.clone());

                spawn_lsp_request(reply, future, move |resp: Option<Vec<helix_lsp_types::SymbolInformation>>| {
                    let symbols = resp
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|si| {
                            let path_abs = si.location.uri.to_file_path().ok()?;
                            let path_rel = path_abs
                                .strip_prefix(&workspace_clone)
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_else(|_| path_abs.to_string_lossy().into_owned());
                            Some(helix_context_schema::LspSymbolInfo {
                                name: si.name,
                                kind: lsp_symbol_kind_to_string(si.kind),
                                location: helix_context_schema::LspLocation {
                                    path: path_rel,
                                    path_abs: path_abs.to_string_lossy().into_owned(),
                                    range: helix_context_schema::LspRange {
                                        start: helix_context_schema::LspPosition {
                                            line: si.location.range.start.line,
                                            character: si.location.range.start.character,
                                        },
                                        end: helix_context_schema::LspPosition {
                                            line: si.location.range.end.line,
                                            character: si.location.range.end.character,
                                        },
                                    },
                                },
                                container_name: si.container_name,
                            })
                        })
                        .collect();
                    helix_context_schema::ControlResponse::GetWorkspaceSymbols { symbols }
                });
                return;
            }
```

Add the `SymbolKind → string` helper:

```rust
fn lsp_symbol_kind_to_string(kind: helix_lsp_types::SymbolKind) -> String {
    use helix_lsp_types::SymbolKind;
    match kind {
        SymbolKind::FILE => "file",
        SymbolKind::MODULE => "module",
        SymbolKind::NAMESPACE => "namespace",
        SymbolKind::PACKAGE => "package",
        SymbolKind::CLASS => "class",
        SymbolKind::METHOD => "method",
        SymbolKind::PROPERTY => "property",
        SymbolKind::FIELD => "field",
        SymbolKind::CONSTRUCTOR => "constructor",
        SymbolKind::ENUM => "enum",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::FUNCTION => "function",
        SymbolKind::VARIABLE => "variable",
        SymbolKind::CONSTANT => "constant",
        SymbolKind::STRING => "string",
        SymbolKind::NUMBER => "number",
        SymbolKind::BOOLEAN => "boolean",
        SymbolKind::ARRAY => "array",
        SymbolKind::OBJECT => "object",
        SymbolKind::KEY => "key",
        SymbolKind::NULL => "null",
        SymbolKind::ENUM_MEMBER => "enum_member",
        SymbolKind::STRUCT => "struct",
        SymbolKind::EVENT => "event",
        SymbolKind::OPERATOR => "operator",
        SymbolKind::TYPE_PARAMETER => "type_parameter",
        _ => "unknown",
    }
    .into()
}
```

- [ ] **Step 3: Verify + tests**

Run: `cargo check --workspace`
Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 44 + 14 pass.

- [ ] **Step 4: Smoke test (references + symbols together)**

```bash
mkdir -p /tmp/p3-rest && cd /tmp/p3-rest && git init -q
cat > Cargo.toml <<'EOF'
[package]
name = "p3-rest"
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
    let y = helper();
    println!("{}", x + y);
}
EOF

cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 5
SOCK=$(ls /tmp/p3-rest/.helix/control-*.sock | head -1)

# References to `helper`
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect('$SOCK')
s.sendall(b'{\"method\":\"get-references-at\",\"params\":{\"line\":1,\"column\":4}}\n')
print('refs:', json.dumps(json.loads(s.recv(8192).decode()), indent=2))
"

# Workspace symbols for `helper`
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect('$SOCK')
s.sendall(b'{\"method\":\"get-workspace-symbols\",\"params\":{\"query\":\"helper\"}}\n')
print('syms:', json.dumps(json.loads(s.recv(8192).decode()), indent=2))
"

pkill -P $HX_PID; kill $HX_PID; sleep 0.5
rm -rf /tmp/p3-rest
```

Expected: references returns ≥2 entries (definition + each call site). workspace-symbols returns the `helper` function.

- [ ] **Step 5: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement get-references-at and get-workspace-symbols

Both use spawn_lsp_request pattern. References calls text_document_references
with the user-provided include_declaration (defaults to true). Workspace
symbols picks the first LSP with WorkspaceSymbols feature from any open
doc and calls workspace_symbols(query).

Result mapping: LSP SymbolKind enum → lowercase string for the schema's
LspSymbolInfo.kind field. Avoids leaking lsp-types into clients.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.2)
EOF
)"
```

---

## Task 9: Advertise all five LSP methods + final e2e smoke

**Files:**
- Modify: `helix-term/src/control_socket/dispatch.rs`

- [ ] **Step 1: Update advertised capabilities**

Edit `helix-term/src/control_socket/dispatch.rs`. Find `handle_initialize`. Extend `read_methods`:

```rust
            read_methods: vec![
                "initialize".into(),
                "current-state".into(),
                "get-open-buffers".into(),
                "get-buffer-text".into(),
                "get-diagnostics".into(),
                "get-hover-at".into(),
                "get-definition-at".into(),
                "get-references-at".into(),
                "get-workspace-symbols".into(),
            ],
```

(`write_methods` stays as `["open-file", "goto-line"]`.)

- [ ] **Step 2: Update or add a test**

Edit the existing test (or add a new one) to assert all nine read methods are present:

```rust
    #[test]
    fn initialize_advertises_all_phase_3_read_methods() {
        let req = ControlRequest::Initialize {
            protocol_version: "1.0".into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let resp = try_dispatch_inline(&req).unwrap().unwrap();
        let ControlResponse::Initialize { capabilities, .. } = resp else {
            panic!("expected Initialize response");
        };
        for method in &[
            "initialize",
            "current-state",
            "get-open-buffers",
            "get-buffer-text",
            "get-diagnostics",
            "get-hover-at",
            "get-definition-at",
            "get-references-at",
            "get-workspace-symbols",
        ] {
            assert!(
                capabilities.read_methods.contains(&method.to_string()),
                "missing read method: {}", method
            );
        }
    }
```

- [ ] **Step 3: Run, verify**

Run: `cargo test -p helix-term control_socket`
Expected: 15 pass (14 + 1 new).

- [ ] **Step 4: Final end-to-end smoke (all 11 methods)**

```bash
mkdir -p /tmp/p3-final && cd /tmp/p3-final && git init -q
cat > Cargo.toml <<'EOF'
[package]
name = "p3-final"
version = "0.1.0"
edition = "2021"
[[bin]]
name = "main"
path = "main.rs"
EOF
cat > main.rs <<'EOF'
fn helper() -> u32 { 42 }
fn main() {
    let x: u32 = helper();
    let y: u32 = helper();
    println!("{}", x + y);
}
EOF

cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 6  # LSP needs time
SOCK=$(ls /tmp/p3-final/.helix/control-*.sock | head -1)

python3 <<PYEOF
import socket, json
def call(method, params=None):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect('$SOCK')
    req = {'method': method, 'params': params or {}}
    s.sendall((json.dumps(req) + '\n').encode())
    return json.loads(s.recv(16384).decode())

# All 11 methods at a glance
methods = [
    ('initialize', {'protocol_version': '1.0', 'client_info': {'name': 't', 'version': '0.1'}}),
    ('current-state', {}),
    ('get-open-buffers', {}),
    ('get-buffer-text', {'range': {'start_line': 1, 'end_line': 3}}),
    ('get-diagnostics', {}),
    ('get-hover-at', {'line': 3, 'column': 9}),
    ('get-definition-at', {'line': 3, 'column': 18}),
    ('get-references-at', {'line': 1, 'column': 4}),
    ('get-workspace-symbols', {'query': 'helper'}),
    ('goto-line', {'line': 5, 'column': 1}),
    ('open-file', {'path': 'main.rs'}),
]

for m, p in methods:
    r = call(m, p)
    if 'code' in r:
        print(f'=== {m} ERROR: {r["message"]}')
    else:
        # Show first ~150 chars of result for brevity
        print(f'=== {m} ===')
        print(json.dumps(r.get('result', r), indent=2)[:250])
PYEOF

pkill -P $HX_PID; kill $HX_PID; sleep 0.5
rm -rf /tmp/p3-final
```

Expected: every method returns a successful response (no `code: -32xxx` error). The LSP methods may return empty results if rust-analyzer is still indexing — increase the sleep if so.

- [ ] **Step 5: Commit**

```bash
git add helix-term/src/control_socket/dispatch.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): advertise all Phase 3 LSP methods in capabilities

read_methods grows from 4 to 9: adds get-diagnostics, get-hover-at,
get-definition-at, get-references-at, get-workspace-symbols.

write_methods unchanged (open-file, goto-line).

Final end-to-end smoke covers all 11 methods. With rust-analyzer active
on a small Cargo project, every method returns a non-error response.
The four LSP-future methods are bounded by spawn_lsp_request's 10s
timeout — slow servers surface as LspTimeout, not editor stalls.

Phase 3 complete. Phase 4 (helix-claude-mcp external binary) is next.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6)
EOF
)"
```

---

## Self-review checklist

- [ ] `cargo check --workspace` clean
- [ ] `cargo test -p helix-context-schema` 44 pass (28 prior + 16 new across Tasks 1+2)
- [ ] `cargo test -p helix-term control_socket` 15 pass (14 prior + 1 new from Task 9)
- [ ] `cargo build --release -p helix-term --bin hx` succeeds
- [ ] Final smoke test in Task 9 passes — all 11 methods return successful responses
- [ ] `git log --oneline -12` shows the Phase 3 commits in sequence

## What's NOT in Phase 3

- **`format-document`** — Phase 6 polish.
- **`run-typable-command`** — deferred (Phase 6 or Phase 2d).
- **`helix-claude-mcp` external MCP bridge binary** — Phase 4.
- **Rust `hook` subcommand** — Phase 5.

## Open questions for the implementer

1. **LSP API exactness.** The plan assumes specific method names (`text_document_hover`, `goto_definition`, `text_document_references`, `workspace_symbols`) and types (`HoverContents`, `GotoDefinitionResponse`, `SymbolInformation`, `SymbolKind`). The actual fork may use slightly different names — adjust on the fly. The architectural pattern (extract future → spawn → convert) is what matters; method names and signatures are mechanical.

2. **Encoding handling.** `helix_lsp::util::pos_to_lsp_pos` requires the LSP server's offset encoding. The plan calls `lsp_client.offset_encoding()` — confirm that method exists (it might be `lsp_client.offset_encoding` field or a different getter).

3. **`spawn_lsp_request` generic bounds.** The error type from Helix's LSP machinery is the most likely friction point. The plan uses `helix_lsp::jsonrpc::Error` — adjust if the actual error type is something like `helix_lsp::Error` or `tower_lsp::jsonrpc::Error`. Make `spawn_lsp_request` generic in the error type if needed.

4. **`workspace_symbols` server selection.** Picking "the first LSP supporting WorkspaceSymbols from any open doc" works for single-language projects. Mixed-language projects would benefit from a query-router (multi-server aggregation), but that's overkill for v1.

5. **rust-analyzer in CI/smoke tests.** Smoke tests assume rust-analyzer is on PATH and starts within 5-6 seconds. Slower hardware may need longer sleeps. If the implementer's sandbox can't run rust-analyzer at all, document that the smoke tests need a real workstation to fully exercise.
