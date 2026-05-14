//! `helix-mcp` — MCP bridge that exposes the Helix editor's control
//! socket through the Model Context Protocol to Claude Code.
//!
//! Subcommands:
//! - `serve`: stdio MCP server. Configured in Claude Code via `.mcp.json`.
//! - `hook`: UserPromptSubmit hook handler (Phase 5).

use clap::{Parser, Subcommand};

mod discovery;
mod doctor;
mod hook;
mod resources;
mod rpc_client;
mod serve;
mod tools;

/// Shared serialization point for test threads that mutate process-global
/// env vars (XDG_RUNTIME_DIR, CLAUDE_PROJECT_DIR). Lives at the crate root
/// so every test module that touches the same vars goes through one lock —
/// per-module mutexes wouldn't coordinate across modules.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Parser)]
#[command(name = "helix-mcp", version)]
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
        /// Emit decision breadcrumbs on stderr (parsed input, decision,
        /// emit/marker failures). Diagnostic only — does not change
        /// emission behavior. Without this flag, only WARN/ERROR logs
        /// reach stderr, so debugging "why didn't my snapshot inject"
        /// would otherwise require `RUST_LOG=debug` on the hook entry.
        #[arg(long)]
        verbose: bool,
    },
    /// Run a self-diagnosis: binary on PATH, snapshot present and
    /// parseable, control-socket connectable, initialize handshake.
    /// Prints a five-line report. Useful when onboarding a new install
    /// or debugging "why doesn't Claude see my editor".
    Doctor,
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .target(env_logger::Target::Stderr) // stdout is reserved for MCP framing
    .init();

    let cli = Cli::parse();
    match cli.command {
        // The hook subcommand is pure stdio + std::fs — zero await points.
        // Building a tokio runtime for it would cost ~0.5–1 ms of process
        // startup per UserPromptSubmit (worker-thread pool, io driver) for
        // no benefit. Run it synchronously on the main thread.
        Command::Hook { reset_marker, verbose } => hook::run(reset_marker, verbose),
        // serve and doctor speak async (rmcp stdio transport, tokio Unix
        // sockets for the bridge → Helix RPC). Build a runtime for them.
        Command::Serve => tokio_runtime()?.block_on(serve::run()),
        Command::Doctor => tokio_runtime()?.block_on(doctor::run()),
    }
}

fn tokio_runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
}
