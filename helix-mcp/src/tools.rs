//! Tool definitions.
//!
//! Single source of truth: `TOOLS` is a static slice of `ToolSpec` rows, one
//! per MCP tool exposed by helix-mcp. Each row holds the tool's `ToolKind`
//! discriminant, the MCP-visible name, a description for the LLM, a
//! function pointer to build its JSON schema, and a function pointer that
//! deserializes the tool's MCP arguments into a `ControlRequest`.
//!
//! Adding a new tool: define a new `*Args` struct, write a `parsers::*` and
//! `schemas::*` function, add the `ToolKind` variant + `TOOLS` row. The
//! `ToolKind::name`/`description`/`input_schema`/`from_name`/`all` accessors
//! and the `parse_request` dispatch in `serve.rs::call_tool` derive from
//! this table — no edits needed on those paths.

use helix_context_schema::ControlRequest;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    HelixOpenFile,
    HelixGotoLine,
    HelixSelect,
    HelixGetDiagnostics,
    HelixGetHover,
    HelixGetDefinition,
    HelixGetReferences,
    HelixGetWorkspaceSymbols,
    HelixBufferRead,
    HelixFormatDocument,
    HelixRunCommand,
}

/// One row in the tool table. Every field except `kind` is derived from
/// this row — adding a tool means adding a row plus an args struct and a
/// parser. The schema/parser are function pointers so the table is `const`.
pub struct ToolSpec {
    pub kind: ToolKind,
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
    pub parse_request: fn(Value) -> Result<ControlRequest, serde_json::Error>,
}

pub const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        kind: ToolKind::HelixOpenFile,
        name: "helix_open_file",
        description:
            "Open a file in the running Helix editor and focus it. Path is absolute \
             or relative to the workspace root. Optionally pass line (and column, both \
             1-indexed) to jump and center the view on that position — useful for \
             showing the user exactly where you're about to make changes before \
             calling Edit/Write.",
        schema: schemas::open_file,
        parse_request: parsers::open_file,
    },
    ToolSpec {
        kind: ToolKind::HelixGotoLine,
        name: "helix_goto_line",
        description:
            "Move the cursor in the running Helix editor to a 1-indexed line \
             (and optional column). The view recenters on the target line so the \
             user sees surrounding context. When path is given, switches to that \
             buffer first; the buffer must already be open.",
        schema: schemas::goto_line,
        parse_request: parsers::goto_line,
    },
    ToolSpec {
        kind: ToolKind::HelixSelect,
        name: "helix_select",
        description:
            "Select a range in the running Helix editor's buffer, from \
             (start_line, start_column) to (end_line, end_column), all 1-indexed and \
             inclusive. `start` becomes the anchor and `end` becomes the cursor head; \
             the view scrolls so the selection is centered. Use this to show the user \
             a specific region or to set up a selection for a follow-up `helix_run_command`. \
             When path is given, switches to that buffer first.",
        schema: schemas::select,
        parse_request: parsers::select,
    },
    ToolSpec {
        kind: ToolKind::HelixGetDiagnostics,
        name: "helix_get_diagnostics",
        description:
            "Return all LSP diagnostics for a file (defaults to active buffer). \
             Reads Helix's cached diagnostics; returns empty array if LSP is still \
             analyzing or finds no issues.",
        schema: schemas::get_diagnostics,
        parse_request: parsers::get_diagnostics,
    },
    ToolSpec {
        kind: ToolKind::HelixGetHover,
        name: "helix_get_hover",
        description:
            "Return LSP hover info (type signature, doc) at a 1-indexed (line, column) \
             position. Refuses with BufferModeUnsafe in Insert mode unless \
             allow_insert_mode: true.",
        schema: schemas::position,
        parse_request: parsers::get_hover,
    },
    ToolSpec {
        kind: ToolKind::HelixGetDefinition,
        name: "helix_get_definition",
        description:
            "Return LSP goto-definition locations for a symbol at a 1-indexed \
             (line, column) position. Refuses in Insert mode unless allow_insert_mode: true.",
        schema: schemas::position,
        parse_request: parsers::get_definition,
    },
    ToolSpec {
        kind: ToolKind::HelixGetReferences,
        name: "helix_get_references",
        description:
            "Return all LSP references for a symbol at a 1-indexed (line, column) \
             position. Set include_declaration: true to include the symbol's own \
             declaration site (default: true). Refuses in Insert mode unless \
             allow_insert_mode: true.",
        schema: schemas::get_references,
        parse_request: parsers::get_references,
    },
    ToolSpec {
        kind: ToolKind::HelixGetWorkspaceSymbols,
        name: "helix_get_workspace_symbols",
        description:
            "Fuzzy-search workspace symbols by query string. Returns symbols with \
             kind, name, location.",
        schema: schemas::get_workspace_symbols,
        parse_request: parsers::get_workspace_symbols,
    },
    ToolSpec {
        kind: ToolKind::HelixBufferRead,
        name: "helix_buffer_read",
        description:
            "Read text from a Helix buffer's live (in-memory) rope. Prefer over \
             the standard Read tool when the user may have unsaved edits — the \
             rope reflects what's in the editor right now, not what's on disk. \
             Optional 1-indexed start_line/end_line (inclusive) for a slice; \
             omit both to read the whole buffer. The buffer must already be \
             open; pass `path` to read a specific buffer or omit to read the \
             active one.",
        schema: schemas::buffer_read,
        parse_request: parsers::buffer_read,
    },
    ToolSpec {
        kind: ToolKind::HelixFormatDocument,
        name: "helix_format_document",
        description:
            "Format a buffer using the LSP formatter (rust-analyzer, gopls, etc.). \
             The buffer must already be open; pass path: 'foo.rs' to format a specific \
             buffer, or omit path to format the active one. Returns applied: true when \
             the format was kicked off. The actual edits arrive asynchronously via the \
             LSP response.",
        schema: schemas::format_document,
        parse_request: parsers::format_document,
    },
    ToolSpec {
        kind: ToolKind::HelixRunCommand,
        name: "helix_run_command",
        description:
            "Execute a Helix typable command. Useful for { name: 'write' } to save, \
             { name: 'reload' } to reload from disk, { name: 'format' } to format via \
             the LSP, { name: 'open', args: ['src/main.rs'] } to open a file. By \
             default, a small denylist refuses commands whose damage cannot be undone \
             via normal editing: force-quits (`quit!`, `q!`, `quit-all!`, `qa!`) and \
             shell-execs (`run-shell-command`, `sh`, `bang`, `!`, `pipe`, `pipe-to`). \
             Everything else — `:write`, `:reload`, `:format`, `:theme`, `:set` — \
             remains available. To opt out of the denylist, set \
             `HELIX_CONTROL_SOCKET_ALLOW_DESTRUCTIVE=1` before starting Helix. Pass \
             `name` without the leading colon; pass `args` for additional positionals \
             (each element becomes one token, no shell parsing).",
        schema: schemas::run_command,
        parse_request: parsers::run_command,
    },
];

impl ToolKind {
    fn spec(self) -> &'static ToolSpec {
        // Linear scan over a 10-entry slice — equal cost to a match for this
        // table size, and never appears in a hot path (tools/list runs once
        // per session; tools/call runs at human-typing frequency).
        TOOLS
            .iter()
            .find(|t| t.kind == self)
            .expect("every ToolKind variant must have a TOOLS row — see tools.rs")
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }

    pub fn description(self) -> &'static str {
        self.spec().description
    }

    pub fn input_schema(self) -> Value {
        (self.spec().schema)()
    }

    pub fn parse_request(self, args: Value) -> Result<ControlRequest, serde_json::Error> {
        (self.spec().parse_request)(args)
    }

    pub fn from_name(name: &str) -> Option<Self> {
        TOOLS.iter().find(|t| t.name == name).map(|t| t.kind)
    }

    pub fn all() -> impl Iterator<Item = Self> {
        TOOLS.iter().map(|t| t.kind)
    }
}

// ---------------------------------------------------------------------------
// Args structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct HelixOpenFileArgs {
    pub path: String,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub column: Option<usize>,
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
pub struct HelixSelectArgs {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
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

#[derive(Debug, Deserialize)]
pub struct HelixBufferReadArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct HelixFormatDocumentArgs {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HelixRunCommandArgs {
    pub name: String,
    #[serde(default)]
    pub args: Vec<String>,
}

// ---------------------------------------------------------------------------
// Schemas — one function per tool. JSON-Schema for input validation +
// LLM-facing argument hints. Kept in a submodule so the TOOLS table reads
// cleanly above.
// ---------------------------------------------------------------------------

mod schemas {
    use super::*;

    pub fn open_file() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path, absolute or workspace-relative" },
                "line": { "type": "integer", "minimum": 1, "description": "Optional 1-indexed line to jump to after opening. View recenters on this line." },
                "column": { "type": "integer", "minimum": 1, "description": "Optional 1-indexed column. Ignored unless `line` is also set." }
            },
            "required": ["path"]
        })
    }

    pub fn goto_line() -> Value {
        json!({
            "type": "object",
            "properties": {
                "line": { "type": "integer", "minimum": 1, "description": "1-indexed line number" },
                "column": { "type": "integer", "minimum": 1, "description": "1-indexed column number (optional; defaults to 1)" },
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active buffer). Must already be open." }
            },
            "required": ["line"]
        })
    }

    pub fn select() -> Value {
        json!({
            "type": "object",
            "properties": {
                "start_line": { "type": "integer", "minimum": 1, "description": "Selection start line, 1-indexed" },
                "start_column": { "type": "integer", "minimum": 1, "description": "Selection start column, 1-indexed" },
                "end_line": { "type": "integer", "minimum": 1, "description": "Selection end line, 1-indexed" },
                "end_column": { "type": "integer", "minimum": 1, "description": "Selection end column, 1-indexed" },
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" }
            },
            "required": ["start_line", "start_column", "end_line", "end_column"]
        })
    }

    pub fn get_diagnostics() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" }
            }
        })
    }

    /// Shared between `get_hover` and `get_definition` — both have the same
    /// (line, column, path?, allow_insert_mode?) shape.
    pub fn position() -> Value {
        json!({
            "type": "object",
            "properties": {
                "line": { "type": "integer", "minimum": 1 },
                "column": { "type": "integer", "minimum": 1 },
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" },
                "allow_insert_mode": { "type": "boolean", "description": "Bypass the BufferModeUnsafe refusal" }
            },
            "required": ["line", "column"]
        })
    }

    pub fn get_references() -> Value {
        json!({
            "type": "object",
            "properties": {
                "line": { "type": "integer", "minimum": 1 },
                "column": { "type": "integer", "minimum": 1 },
                "path": { "type": "string" },
                "allow_insert_mode": { "type": "boolean" },
                "include_declaration": { "type": "boolean", "description": "Default: true" }
            },
            "required": ["line", "column"]
        })
    }

    pub fn get_workspace_symbols() -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Fuzzy match query" }
            },
            "required": ["query"]
        })
    }

    pub fn format_document() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" }
            }
        })
    }

    pub fn buffer_read() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" },
                "start_line": { "type": "integer", "minimum": 1, "description": "Optional 1-indexed first line to include (inclusive)" },
                "end_line": { "type": "integer", "minimum": 1, "description": "Optional 1-indexed last line to include (inclusive). Both start_line and end_line must be set to slice." }
            }
        })
    }

    pub fn run_command() -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Typable command name without leading ':'" },
                "args": { "type": "array", "items": { "type": "string" }, "description": "Optional command arguments" }
            },
            "required": ["name"]
        })
    }
}

// ---------------------------------------------------------------------------
// Parsers — one function per tool. Each deserializes the MCP-supplied
// arguments into the tool's `*Args` struct, then maps to the wire-level
// `ControlRequest` variant.
// ---------------------------------------------------------------------------

mod parsers {
    use super::*;

    pub fn open_file(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixOpenFileArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::OpenFile {
            path: a.path,
            line: a.line,
            column: a.column,
        })
    }

    pub fn goto_line(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixGotoLineArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::GotoLine {
            line: a.line,
            column: a.column,
            path: a.path,
        })
    }

    pub fn select(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixSelectArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::SelectRange {
            start_line: a.start_line,
            start_column: a.start_column,
            end_line: a.end_line,
            end_column: a.end_column,
            path: a.path,
        })
    }

    pub fn get_diagnostics(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixGetDiagnosticsArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::GetDiagnostics { path: a.path })
    }

    pub fn get_hover(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixPositionArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::GetHoverAt {
            line: a.line,
            column: a.column,
            path: a.path,
            allow_insert_mode: a.allow_insert_mode,
        })
    }

    pub fn get_definition(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixPositionArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::GetDefinitionAt {
            line: a.line,
            column: a.column,
            path: a.path,
            allow_insert_mode: a.allow_insert_mode,
        })
    }

    pub fn get_references(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixGetReferencesArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::GetReferencesAt {
            line: a.line,
            column: a.column,
            path: a.path,
            allow_insert_mode: a.allow_insert_mode,
            include_declaration: a.include_declaration,
        })
    }

    pub fn get_workspace_symbols(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixGetWorkspaceSymbolsArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::GetWorkspaceSymbols { query: a.query })
    }

    pub fn format_document(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixFormatDocumentArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::FormatDocument { path: a.path })
    }

    pub fn buffer_read(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixBufferReadArgs = serde_json::from_value(v)?;
        // The wire method takes Option<LineRange>; build one only when
        // both endpoints are present. Single-endpoint args are
        // ambiguous (read from line N to end? start to N?), so we
        // require both or neither.
        let range = match (a.start_line, a.end_line) {
            (Some(start_line), Some(end_line)) => {
                Some(helix_context_schema::LineRange { start_line, end_line })
            }
            (None, None) => None,
            _ => {
                return Err(serde::de::Error::custom(
                    "helix_buffer_read requires both start_line and end_line, or neither",
                ));
            }
        };
        Ok(ControlRequest::GetBufferText { path: a.path, range })
    }

    pub fn run_command(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixRunCommandArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::RunCommand {
            name: a.name,
            args: a.args,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time-ish exhaustiveness: every variant constructed in this
    /// list must be present in TOOLS, or `.spec()` would panic. If a new
    /// ToolKind variant is added without a corresponding TOOLS row, this
    /// test fails before any other test runs.
    #[test]
    fn every_tool_kind_has_a_spec_row() {
        let all_variants = [
            ToolKind::HelixOpenFile,
            ToolKind::HelixGotoLine,
            ToolKind::HelixSelect,
            ToolKind::HelixGetDiagnostics,
            ToolKind::HelixGetHover,
            ToolKind::HelixGetDefinition,
            ToolKind::HelixGetReferences,
            ToolKind::HelixGetWorkspaceSymbols,
            ToolKind::HelixBufferRead,
            ToolKind::HelixFormatDocument,
            ToolKind::HelixRunCommand,
        ];
        for kind in all_variants {
            // .spec() asserts the row exists; if a variant were missing
            // from TOOLS this would panic.
            let _ = kind.name();
        }
        assert_eq!(TOOLS.len(), all_variants.len());
    }

    #[test]
    fn all_iterates_eleven_kinds() {
        let kinds: Vec<_> = ToolKind::all().collect();
        assert_eq!(kinds.len(), 11);
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
    fn names_are_unique() {
        let names: Vec<_> = TOOLS.iter().map(|t| t.name).collect();
        let mut deduped = names.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(names.len(), deduped.len(), "duplicate tool name in TOOLS");
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
        assert!(a.line.is_none());
        assert!(a.column.is_none());
    }

    #[test]
    fn open_file_args_with_line_and_column() {
        let v = json!({"path": "src/main.rs", "line": 42, "column": 5});
        let a: HelixOpenFileArgs = serde_json::from_value(v).unwrap();
        assert_eq!(a.path, "src/main.rs");
        assert_eq!(a.line, Some(42));
        assert_eq!(a.column, Some(5));
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

    #[test]
    fn parse_request_open_file_round_trip() {
        let v = json!({"path": "src/main.rs", "line": 10, "column": 1});
        let req = ToolKind::HelixOpenFile.parse_request(v).unwrap();
        match req {
            ControlRequest::OpenFile { path, line, column } => {
                assert_eq!(path, "src/main.rs");
                assert_eq!(line, Some(10));
                assert_eq!(column, Some(1));
            }
            other => panic!("expected OpenFile, got {:?}", other),
        }
    }

    #[test]
    fn parse_request_select_round_trip() {
        let v = json!({"start_line":1, "start_column":1, "end_line":5, "end_column":10});
        let req = ToolKind::HelixSelect.parse_request(v).unwrap();
        match req {
            ControlRequest::SelectRange { start_line, end_line, .. } => {
                assert_eq!(start_line, 1);
                assert_eq!(end_line, 5);
            }
            other => panic!("expected SelectRange, got {:?}", other),
        }
    }

    #[test]
    fn parse_request_returns_err_on_bad_args() {
        let v = json!({"path": "x"}); // missing required line for goto_line
        let result = ToolKind::HelixGotoLine.parse_request(v);
        assert!(result.is_err());
    }
}
