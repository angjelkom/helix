use serde::{Deserialize, Serialize};

/// What caused the most recent snapshot write.
///
/// - `FocusLost`: the user switched away from the terminal (default writer).
/// - `McpCommand`: an external tool (the MCP bridge) mutated editor state.
///   Hook readers should treat this as "Claude already knows" and skip injection.
/// - `Manual`: the user explicitly ran the `:write-context` typable command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateSource {
    FocusLost,
    McpCommand,
    Manual,
}
