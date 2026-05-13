//! `serve` subcommand: stdio MCP server.
//!
//! Reads MCP protocol on stdin, writes responses on stdout. stderr is
//! reserved for logs (env_logger). Run by Claude Code via the `.mcp.json`
//! config in a project workspace.

use anyhow::Result;

pub async fn run() -> Result<()> {
    log::info!("starting stdio MCP server");

    // PLACEHOLDER — Task 5 fills in the real rmcp server wiring.
    // For Task 4 this stub is intentional: it verifies the module compiles
    // and the dispatch path is wired correctly.
    anyhow::bail!("rmcp server wiring TODO — implementer to complete in Task 5")
}
