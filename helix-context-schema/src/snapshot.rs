use serde::{Deserialize, Serialize};

use crate::types::{Active, Instance, OpenBuffer};
use crate::UpdateSource;

/// The full JSON shape of `<workspace>/.helix/context.json`.
///
/// Producer: `helix-term::context_logger` (and, in Phase 2+, the MCP bridge
/// via the control socket).
/// Consumer: the Claude Code UserPromptSubmit hook (today: shell; later: a
/// `helix-claude-mcp hook` subcommand).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub schema_version: u32,
    pub min_supported_reader: u32,
    pub timestamp: String,
    pub last_update_source: UpdateSource,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub instance: Option<Instance>,
    pub project_root: String,
    pub mode: String,
    pub active: Active,
    pub open_buffers: Vec<OpenBuffer>,
}
