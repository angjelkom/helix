//! `hook` subcommand — Claude Code UserPromptSubmit handler.
//!
//! Wired in `~/.claude/settings.json` under `hooks.UserPromptSubmit` and
//! (with `--reset-marker`) under `hooks.PostCompact` plus `SessionStart`
//! with `matcher: "compact"`. See `README.md`.
//!
//! Replaces the shell hook at `~/.claude/hooks/helix-context.sh`.

use anyhow::Result;

pub async fn run(_reset_marker: bool) -> Result<()> {
    // Tasks 2-5 fill in the implementation. The skeleton exists so main.rs
    // compiles and the clap subcommand is reachable.
    Ok(())
}
