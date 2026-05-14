//! Tool definitions. One enum variant per MCP Tool exposed by helix-mcp.
//! Each variant carries its public name, description, and JSON-Schema input
//! shape. The arg structs (HelixOpenFileArgs, etc.) live alongside for
//! ergonomic deserialization in serve::call_tool.

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
    HelixFormatDocument,
    HelixRunCommand,
}

impl ToolKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::HelixOpenFile => "helix_open_file",
            Self::HelixGotoLine => "helix_goto_line",
            Self::HelixSelect => "helix_select",
            Self::HelixGetDiagnostics => "helix_get_diagnostics",
            Self::HelixGetHover => "helix_get_hover",
            Self::HelixGetDefinition => "helix_get_definition",
            Self::HelixGetReferences => "helix_get_references",
            Self::HelixGetWorkspaceSymbols => "helix_get_workspace_symbols",
            Self::HelixFormatDocument => "helix_format_document",
            Self::HelixRunCommand => "helix_run_command",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::HelixOpenFile => {
                "Open a file in the running Helix editor and focus it. Path is absolute \
                 or relative to the workspace root. Optionally pass line (and column, both \
                 1-indexed) to jump and center the view on that position — useful for \
                 showing the user exactly where you're about to make changes before \
                 calling Edit/Write."
            }
            Self::HelixGotoLine => {
                "Move the cursor in the running Helix editor to a 1-indexed line \
                 (and optional column). The view recenters on the target line so the \
                 user sees surrounding context. When path is given, switches to that \
                 buffer first; the buffer must already be open."
            }
            Self::HelixSelect => {
                "Select a range in the running Helix editor's buffer, from \
                 (start_line, start_column) to (end_line, end_column), all 1-indexed and \
                 inclusive. `start` becomes the anchor and `end` becomes the cursor head; \
                 the view scrolls so the selection is centered. Use this to show the user \
                 a specific region or to set up a selection for a follow-up `helix_run_command`. \
                 When path is given, switches to that buffer first."
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
            Self::HelixFormatDocument => {
                "Format a buffer using the LSP formatter (rust-analyzer, gopls, etc.). \
                 The buffer must already be open; pass path: 'foo.rs' to format a specific \
                 buffer, or omit path to format the active one. Returns applied: true when \
                 the format was kicked off. The actual edits arrive asynchronously via the \
                 LSP response."
            }
            Self::HelixRunCommand => {
                "Execute an arbitrary Helix typable command. POWERFUL — can do anything a \
                 user can type at the `:` prompt: write files, reload config, run shell \
                 commands via `:run-shell-command`, etc. Pass name as the command without \
                 the leading `:`. Use args for additional arguments (joined with spaces). \
                 Examples: { name: 'write' } to save; { name: 'reload' } to reload from \
                 disk; { name: 'open', args: ['src/main.rs'] } to open a file."
            }
        }
    }

    pub fn input_schema(self) -> Value {
        match self {
            Self::HelixOpenFile => json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path, absolute or workspace-relative" },
                    "line": { "type": "integer", "minimum": 1, "description": "Optional 1-indexed line to jump to after opening. View recenters on this line." },
                    "column": { "type": "integer", "minimum": 1, "description": "Optional 1-indexed column. Ignored unless `line` is also set." }
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
            Self::HelixSelect => json!({
                "type": "object",
                "properties": {
                    "start_line": { "type": "integer", "minimum": 1, "description": "Selection start line, 1-indexed" },
                    "start_column": { "type": "integer", "minimum": 1, "description": "Selection start column, 1-indexed" },
                    "end_line": { "type": "integer", "minimum": 1, "description": "Selection end line, 1-indexed" },
                    "end_column": { "type": "integer", "minimum": 1, "description": "Selection end column, 1-indexed" },
                    "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" }
                },
                "required": ["start_line", "start_column", "end_line", "end_column"]
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
            Self::HelixFormatDocument => json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" }
                }
            }),
            Self::HelixRunCommand => json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Typable command name without leading ':'" },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "Optional command arguments" }
                },
                "required": ["name"]
            }),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "helix_open_file" => Some(Self::HelixOpenFile),
            "helix_goto_line" => Some(Self::HelixGotoLine),
            "helix_select" => Some(Self::HelixSelect),
            "helix_get_diagnostics" => Some(Self::HelixGetDiagnostics),
            "helix_get_hover" => Some(Self::HelixGetHover),
            "helix_get_definition" => Some(Self::HelixGetDefinition),
            "helix_get_references" => Some(Self::HelixGetReferences),
            "helix_get_workspace_symbols" => Some(Self::HelixGetWorkspaceSymbols),
            "helix_format_document" => Some(Self::HelixFormatDocument),
            "helix_run_command" => Some(Self::HelixRunCommand),
            _ => None,
        }
    }

    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::HelixOpenFile,
            Self::HelixGotoLine,
            Self::HelixSelect,
            Self::HelixGetDiagnostics,
            Self::HelixGetHover,
            Self::HelixGetDefinition,
            Self::HelixGetReferences,
            Self::HelixGetWorkspaceSymbols,
            Self::HelixFormatDocument,
            Self::HelixRunCommand,
        ]
        .into_iter()
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_iterates_ten_kinds() {
        let kinds: Vec<_> = ToolKind::all().collect();
        assert_eq!(kinds.len(), 10);
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
}
