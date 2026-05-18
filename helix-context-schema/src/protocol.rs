//! Protocol types for the Helix control socket. JSON-RPC-inspired
//! request/response wire format, but **not** strictly JSON-RPC 2.0:
//!
//! - One request per newline-delimited line, one response per line.
//! - One request at a time per connection — no pipelining.
//! - No `jsonrpc: "2.0"` envelope field on the wire.
//! - No `id` field — request-response order is preserved by the connection's
//!   sequential read/write loop, so correlation is unnecessary.
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

/// A 1-indexed, inclusive line range. Matches the indexing used throughout
/// the snapshot schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub start_line: usize,
    pub end_line: usize,
}

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

/// One signature returned by `GetSignatureHelp`. Mirrors LSP's
/// `SignatureInformation` with documentation flattened from
/// `MarkupContent`/`String` variants to plain text, and parameter
/// labels resolved to strings (LSP allows offset-into-label form;
/// we resolve at the bridge so consumers don't need to).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSignatureInfo {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub documentation: Option<String>,
    #[serde(default)]
    pub parameters: Vec<LspParameterInfo>,
    /// Per-signature active-parameter index (LSP 3.16+). When set,
    /// overrides the top-level `active_parameter` for this signature.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_parameter: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspParameterInfo {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub documentation: Option<String>,
}

/// Nested file outline returned by `GetDocumentSymbols`. The LSP can
/// return either flat `SymbolInformation` lists or nested
/// `DocumentSymbol` trees; this type always carries the nested form
/// (flat results are wrapped with empty `children`). Mirrors LSP's
/// `DocumentSymbol` shape but kept here to avoid a cross-crate
/// dependency on the LSP types crate from `helix-context-schema`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSymbol {
    pub name: String,
    /// LSP SymbolKind as a lowercase string ("function", "class", …).
    pub kind: String,
    /// Full extent (declaration + body).
    pub range: LspRange,
    /// Subrange to highlight when navigating to this symbol —
    /// typically just the name.
    pub selection_range: LspRange,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub container_name: Option<String>,
    #[serde(default)]
    pub children: Vec<DocumentSymbol>,
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
    CurrentState {},
    GetOpenBuffers {},
    GetBufferText {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        range: Option<LineRange>,
    },
    OpenFile {
        path: String,
        /// Optional 1-indexed line to jump to after opening. View recenters
        /// on this line. When omitted, the cursor lands wherever it last
        /// was in the file (or line 1 for a new buffer).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        line: Option<usize>,
        /// Optional 1-indexed column. Ignored unless `line` is also set.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        column: Option<usize>,
    },
    GotoLine {
        line: usize,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        column: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
    },
    /// Replace the buffer's selection with a single range from
    /// (`start_line`, `start_column`) to (`end_line`, `end_column`),
    /// 1-indexed inclusive. `start` becomes the anchor; `end` becomes
    /// the head (cursor position). Columns are clamped to line length.
    /// View is recentered on the head so the user sees what was selected.
    SelectRange {
        start_line: usize,
        start_column: usize,
        end_line: usize,
        end_column: usize,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
    },
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
    /// LSP signature help at a 1-indexed (line, column) position. Use
    /// when writing a function call to surface the LSP-resolved
    /// argument order. Defaults `allow_insert_mode: true` because
    /// signature help is most useful mid-typing — unlike hover/definition,
    /// the LSP is designed to be called between keystrokes.
    GetSignatureHelp {
        line: usize,
        column: usize,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        allow_insert_mode: Option<bool>,
    },
    /// LSP-backed file outline. Returns the symbol tree (nested) for
    /// the named buffer or the active one. Unique value over
    /// `GetWorkspaceSymbols`: workspace-symbols is fuzzy global search;
    /// this is the structured outline of one file.
    GetDocumentSymbols {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
    },
    /// Return the live selections in the current (or named) buffer's
    /// active view, with rope-extracted text for each. The snapshot
    /// carries selections as `(line, column)` pairs only; this method
    /// includes the actual selected text — useful when the user says
    /// "fix the selected region" without quoting the content.
    GetSelections {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
    },
    FormatDocument {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
    },
    RunCommand {
        name: String,
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        args: Vec<String>,
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
    CurrentState {
        active: crate::types::Active,
        mode: String,
    },
    GetOpenBuffers {
        buffers: Vec<crate::types::OpenBuffer>,
    },
    GetBufferText {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        language: Option<String>,
        line_count: usize,
    },
    /// Generic success response for state-mutating methods (open-file,
    /// goto-line). Carries no payload — the client used the tool, the tool
    /// worked, that's all there is to say.
    Ok {},
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
    GetDocumentSymbols {
        symbols: Vec<DocumentSymbol>,
    },
    GetSignatureHelp {
        signatures: Vec<LspSignatureInfo>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        active_signature: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        active_parameter: Option<u32>,
    },
    /// Response to `GetSelections`. Each entry has the anchor/head as
    /// 1-indexed positions plus the rope-extracted text. `primary_index`
    /// points at which entry in `selections` is the primary cursor.
    /// `mode` is the current editor mode ("normal", "select", …) so
    /// the LLM can interpret the selection meaningfully.
    GetSelections {
        selections: Vec<crate::types::Selection>,
        primary_index: usize,
        mode: String,
    },
    FormatDocument {
        applied: bool,
    },
    RunCommand {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        message: Option<String>,
    },
}
