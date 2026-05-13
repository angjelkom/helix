//! `helix-claude-mcp` — MCP bridge that exposes the Helix editor's control
//! socket through the Model Context Protocol to Claude Code.
//!
//! Subcommands:
//! - `serve`: stdio MCP server. Configured in Claude Code via `.mcp.json`.
//! - `hook`: UserPromptSubmit hook handler (Phase 5).

use clap::{Parser, Subcommand};

mod discovery;
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
    /// Run the UserPromptSubmit hook (Phase 5; not yet implemented).
    Hook,
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
        Command::Hook => {
            anyhow::bail!("hook is a Phase 5 deliverable");
        }
    }
}
