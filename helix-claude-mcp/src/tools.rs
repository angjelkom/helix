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
