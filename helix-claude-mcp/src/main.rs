//! `helix-claude-mcp` — MCP bridge that exposes the Helix editor's control
//! socket through the Model Context Protocol to Claude Code.
//!
//! Subcommands:
//! - `serve`: stdio MCP server. Configured in Claude Code via `.mcp.json`.
//! - `hook`: UserPromptSubmit hook handler (Phase 5).

use clap::{Parser, Subcommand};

mod discovery;
mod hook;
mod resources;
mod rpc_client;
mod serve;
mod tools;

#[derive(Parser)]
#[command(name = "helix-claude-mcp", version)]
#[command(about = "MCP bridge for the Helix editor's control socket")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the stdio MCP server. Configured in Claude Code's .mcp.json.
    Serve,
    /// Run as a Claude Code hook. Without arguments: UserPromptSubmit
    /// handler — read stdin JSON, emit wrapped snapshot if appropriate.
    /// With --reset-marker: clear the session's mtime marker so the
    /// next UserPromptSubmit re-injects. Used by PostCompact and
    /// SessionStart matcher=compact.
    Hook {
        /// Clear the per-session marker file (use after context compaction)
        #[arg(long)]
        reset_marker: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .target(env_logger::Target::Stderr) // stdout is reserved for MCP framing
    .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Serve => {
            serve::run().await?;
            Ok(())
        }
        Command::Hook { reset_marker } => {
            hook::run(reset_marker).await
        }
    }
}
