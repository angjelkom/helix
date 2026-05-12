use serde::{Deserialize, Serialize};

/// 1-indexed line/column position. Lines and columns are 1-indexed by convention
/// to match user-visible display in Helix's statusline; internally Helix uses
/// 0-indexed positions, so producers must add 1 before serializing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// A single cursor position. Snapshots include every cursor in a multi-cursor
/// selection, with one marked `primary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    pub primary: bool,
    pub line: usize,
    pub column: usize,
}

/// A visual selection range (only emitted when the user has selected more
/// than one character — single-cursor zero-width ranges are excluded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub primary: bool,
    pub start: Position,
    pub end: Position,
    pub byte_len: usize,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
}

/// Metadata about each open buffer in Helix (not just the active one).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenBuffer {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<String>,
    pub modified: bool,
}

/// State of the currently focused buffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Active {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path_abs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<String>,
    pub modified: bool,
    pub line_count: usize,
    pub cursors: Vec<Cursor>,
    pub selections: Vec<Selection>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
}
