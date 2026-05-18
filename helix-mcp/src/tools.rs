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
    HelixMultiSelect,
    HelixGetDiagnostics,
    HelixGetHover,
    HelixGetDefinition,
    HelixGetReferences,
    HelixGetWorkspaceSymbols,
    HelixGetDocumentSymbols,
    HelixGetSignatureHelp,
    HelixGetSelection,
    HelixBufferRead,
    HelixGetJumplist,
    HelixJump,
    HelixGetCodeActions,
    HelixApplyCodeAction,
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
        kind: ToolKind::HelixMultiSelect,
        name: "helix_multi_select",
        description:
            "Replace the buffer's selection with N ranges. Helix's whole editing \
             model is multi-selection — every command operates on it — so this \
             unlocks structural edits a single helix_select can't express \
             (e.g., select every Foo::new() in the file, then helix_run_command \
             to replace them all). Pass `ranges` as an array of \
             { start_line, start_column, end_line, end_column } (all 1-indexed \
             inclusive). `primary_index` (default 0) selects which range becomes \
             the primary cursor. Overlapping ranges are auto-merged by Helix.",
        schema: schemas::multi_select,
        parse_request: parsers::multi_select,
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
        kind: ToolKind::HelixGetDocumentSymbols,
        name: "helix_get_document_symbols",
        description:
            "Return the structured outline of a file from its attached LSP \
             (functions, methods, classes, fields…). Unique value over \
             helix_get_workspace_symbols: workspace-symbols is fuzzy global \
             search; this is the outline of one file. Unique value over \
             grep: tree-aware, structured, includes type kind and \
             declaration vs definition ranges — grep can't tell you that. \
             Returns a nested tree; children belong to their parent's range.",
        schema: schemas::get_document_symbols,
        parse_request: parsers::get_document_symbols,
    },
    ToolSpec {
        kind: ToolKind::HelixGetSignatureHelp,
        name: "helix_get_signature_help",
        description:
            "LSP signature help at a 1-indexed (line, column) position. Returns \
             function-call overloads with parameter labels and the active \
             parameter index. Unlike hover, this is designed to be called \
             mid-typing (allow_insert_mode defaults to true). Use when writing \
             a function call and you need the LSP-resolved argument order in \
             context — most useful for languages with overloaded functions, \
             complex generics, or builder patterns where hover docs aren't \
             enough.",
        schema: schemas::get_signature_help,
        parse_request: parsers::get_signature_help,
    },
    ToolSpec {
        kind: ToolKind::HelixGetSelection,
        name: "helix_get_selection",
        description:
            "Return the live selections in the current (or named) buffer's \
             active view, with rope-extracted text for each range. Use when \
             the user says 'fix the selected region' or 'rename what I \
             highlighted' — the editor knows the live selection contents, \
             which the snapshot's coordinates-only representation can't \
             give you. Per-range text capped at 64 KiB; longer ranges are \
             truncated with a marker. Includes the editor mode so the LLM \
             can interpret select-vs-normal-mode semantics.",
        schema: schemas::get_selection,
        parse_request: parsers::get_selection,
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
        kind: ToolKind::HelixGetJumplist,
        name: "helix_get_jumplist",
        description:
            "Return the active view's jumplist — an ordered history of cursor \
             positions the user has jumped to or from (LSP go-to-definition, \
             window splits, etc.). Each entry has path, line, column, and \
             `is_current` flagging where the user is now. Use to retrace where \
             the user has been, or pair with helix_jump to step back/forward.",
        schema: schemas::no_args,
        parse_request: parsers::get_jumplist,
    },
    ToolSpec {
        kind: ToolKind::HelixJump,
        name: "helix_jump",
        description:
            "Step along the active view's jumplist by `offset` entries. \
             Negative goes backward through history (like Ctrl-O in vim), \
             positive forward (like Ctrl-I). `offset: -1` is the typical \
             'go back' action. View recenters on the destination. Returns \
             Ok regardless of whether the bound was reached — use \
             helix_get_jumplist afterward to confirm position.",
        schema: schemas::jump,
        parse_request: parsers::jump,
    },
    ToolSpec {
        kind: ToolKind::HelixGetCodeActions,
        name: "helix_get_code_actions",
        description:
            "List LSP code actions available at a 1-indexed (line, column) or \
             range. Returns an ordered list with opaque `id`s and human-readable \
             `title`s; `is_preferred: true` is the LSP's signal that this is the \
             obvious fix. Pair with `helix_apply_code_action(id)` to apply one. \
             Use when fixing a diagnostic — the LSP-generated transform is \
             always more accurate than a hand-edit. Pass `only` to filter by \
             kind (\"quickfix\", \"refactor.extract\", \"source.organizeImports\", \
             …). Refuses in Insert mode unless allow_insert_mode: true.",
        schema: schemas::get_code_actions,
        parse_request: parsers::get_code_actions,
    },
    ToolSpec {
        kind: ToolKind::HelixApplyCodeAction,
        name: "helix_apply_code_action",
        description:
            "Apply a code action previously returned by `helix_get_code_actions`, \
             addressed by its opaque `action_id`. Returns `applied: true` and the \
             action title on success. Returns -32006 CodeActionStale if the buffer \
             changed since the id was issued (call get_code_actions again). \
             Returns -32007 CodeActionUnknown if the id was never issued or has \
             been evicted from the bridge's bounded cache.",
        schema: schemas::apply_code_action,
        parse_request: parsers::apply_code_action,
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
pub struct HelixMultiSelectArgs {
    pub ranges: Vec<helix_context_schema::RangeSpec>,
    #[serde(default)]
    pub primary_index: Option<usize>,
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
pub struct HelixGetDocumentSymbolsArgs {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HelixGetSignatureHelpArgs {
    pub line: usize,
    pub column: usize,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub allow_insert_mode: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct HelixGetSelectionArgs {
    #[serde(default)]
    pub path: Option<String>,
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
pub struct HelixGetJumplistArgs {}

#[derive(Debug, Deserialize)]
pub struct HelixJumpArgs {
    pub offset: i32,
}

#[derive(Debug, Deserialize)]
pub struct HelixGetCodeActionsArgs {
    pub line: usize,
    pub column: usize,
    #[serde(default)]
    pub end_line: Option<usize>,
    #[serde(default)]
    pub end_column: Option<usize>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub only: Option<Vec<String>>,
    #[serde(default)]
    pub allow_insert_mode: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct HelixApplyCodeActionArgs {
    pub action_id: String,
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

    pub fn multi_select() -> Value {
        json!({
            "type": "object",
            "properties": {
                "ranges": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "start_line": { "type": "integer", "minimum": 1 },
                            "start_column": { "type": "integer", "minimum": 1 },
                            "end_line": { "type": "integer", "minimum": 1 },
                            "end_column": { "type": "integer", "minimum": 1 }
                        },
                        "required": ["start_line", "start_column", "end_line", "end_column"]
                    }
                },
                "primary_index": { "type": "integer", "minimum": 0, "description": "Default 0; index of the range that becomes the primary cursor" },
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" }
            },
            "required": ["ranges"]
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

    pub fn get_selection() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" }
            }
        })
    }

    pub fn get_document_symbols() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active). Must already be open and attached to an LSP." }
            }
        })
    }

    pub fn get_signature_help() -> Value {
        json!({
            "type": "object",
            "properties": {
                "line": { "type": "integer", "minimum": 1, "description": "1-indexed line number" },
                "column": { "type": "integer", "minimum": 1, "description": "1-indexed column number" },
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" },
                "allow_insert_mode": { "type": "boolean", "description": "Default true (signature help is designed for mid-typing). Pass false to opt back into the BufferModeUnsafe refusal." }
            },
            "required": ["line", "column"]
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

    /// Schema for tools that take no arguments. `properties: {}` keeps
    /// `tools/list` clients happy that always expect an object.
    pub fn no_args() -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    pub fn jump() -> Value {
        json!({
            "type": "object",
            "properties": {
                "offset": {
                    "type": "integer",
                    "description": "How many jumplist entries to traverse. Negative goes backward (-1 = go back one step), positive forward."
                }
            },
            "required": ["offset"]
        })
    }

    pub fn get_code_actions() -> Value {
        json!({
            "type": "object",
            "properties": {
                "line": { "type": "integer", "minimum": 1 },
                "column": { "type": "integer", "minimum": 1 },
                "end_line": { "type": "integer", "minimum": 1, "description": "Optional: pair with end_column to query over a range instead of a point." },
                "end_column": { "type": "integer", "minimum": 1 },
                "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" },
                "only": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional CodeActionKind filter: e.g. [\"quickfix\"], [\"refactor.extract\"], [\"source.organizeImports\"]"
                },
                "allow_insert_mode": { "type": "boolean", "description": "Bypass the BufferModeUnsafe refusal in Insert mode" }
            },
            "required": ["line", "column"]
        })
    }

    pub fn apply_code_action() -> Value {
        json!({
            "type": "object",
            "properties": {
                "action_id": { "type": "string", "description": "Opaque id from a prior helix_get_code_actions call" }
            },
            "required": ["action_id"]
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

    pub fn multi_select(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixMultiSelectArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::SelectMulti {
            ranges: a.ranges,
            primary_index: a.primary_index,
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

    pub fn get_selection(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixGetSelectionArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::GetSelections { path: a.path })
    }

    pub fn get_document_symbols(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixGetDocumentSymbolsArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::GetDocumentSymbols { path: a.path })
    }

    pub fn get_signature_help(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixGetSignatureHelpArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::GetSignatureHelp {
            line: a.line,
            column: a.column,
            path: a.path,
            allow_insert_mode: a.allow_insert_mode,
        })
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

    pub fn get_jumplist(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let _: HelixGetJumplistArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::GetJumplist {})
    }

    pub fn jump(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixJumpArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::Jump { offset: a.offset })
    }

    pub fn get_code_actions(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixGetCodeActionsArgs = serde_json::from_value(v)?;
        // Caller may pass end_line without end_column (or vice versa); the
        // wire method already treats either-missing as "point query".
        // Stricter validation would make the tool brittle for no win.
        Ok(ControlRequest::GetCodeActions {
            line: a.line,
            column: a.column,
            end_line: a.end_line,
            end_column: a.end_column,
            path: a.path,
            only: a.only,
            allow_insert_mode: a.allow_insert_mode,
        })
    }

    pub fn apply_code_action(v: Value) -> Result<ControlRequest, serde_json::Error> {
        let a: HelixApplyCodeActionArgs = serde_json::from_value(v)?;
        Ok(ControlRequest::ApplyCodeAction { action_id: a.action_id })
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
            ToolKind::HelixMultiSelect,
            ToolKind::HelixGetDiagnostics,
            ToolKind::HelixGetHover,
            ToolKind::HelixGetDefinition,
            ToolKind::HelixGetReferences,
            ToolKind::HelixGetWorkspaceSymbols,
            ToolKind::HelixGetDocumentSymbols,
            ToolKind::HelixGetSignatureHelp,
            ToolKind::HelixGetSelection,
            ToolKind::HelixBufferRead,
            ToolKind::HelixGetJumplist,
            ToolKind::HelixJump,
            ToolKind::HelixGetCodeActions,
            ToolKind::HelixApplyCodeAction,
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
    fn all_iterates_nineteen_kinds() {
        let kinds: Vec<_> = ToolKind::all().collect();
        assert_eq!(kinds.len(), 19);
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
