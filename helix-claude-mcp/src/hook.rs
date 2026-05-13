//! `hook` subcommand — Claude Code UserPromptSubmit handler.
//!
//! Wired in `~/.claude/settings.json` under `hooks.UserPromptSubmit` and
//! (with `--reset-marker`) under `hooks.PostCompact` plus `SessionStart`
//! with `matcher: "compact"`. See `README.md`.
//!
//! Replaces the shell hook at `~/.claude/hooks/helix-context.sh`.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Deserialize;

/// Subset of the fields Claude Code passes to hook commands on stdin.
/// serde ignores unknown fields (`hook_event_name`, `transcript_path`,
/// `permission_mode`, ...) by default so we only declare what we use.
#[derive(Debug, Deserialize)]
pub struct HookInput {
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub hook_event_name: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
}

impl HookInput {
    /// Parse from a stdin-style reader (whole document, not framed).
    pub fn parse<R: io::Read>(reader: R) -> Result<Self> {
        Ok(serde_json::from_reader(reader)?)
    }
}

/// Resolve the directory that holds per-session marker files. Cross-platform:
/// - Linux: $XDG_RUNTIME_DIR/claude-helix/ if set, else ~/.cache/claude-helix/
/// - macOS: $XDG_RUNTIME_DIR/claude-helix/ if set, else ~/Library/Caches/claude-helix/,
///   else ~/.cache/claude-helix/
/// - Other: ~/.cache/claude-helix/
pub fn marker_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(xdg).join("claude-helix");
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Caches")
                .join("claude-helix");
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("claude-helix");
    }
    PathBuf::from("/tmp/claude-helix") // last-ditch; should never happen
}

pub fn marker_path(session_id: &str) -> PathBuf {
    marker_dir().join(format!("marker-{}", session_id))
}

pub fn read_marker_mtime(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub fn write_marker_mtime(path: &Path, mtime: u64) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Be permissive with errors here — the dir may already be 0700
            // or we may not own it (shared XDG_RUNTIME_DIR on a multi-user box).
            let _ = std::fs::set_permissions(
                parent,
                std::fs::Permissions::from_mode(0o700),
            );
        }
    }
    std::fs::write(path, mtime.to_string())
}

pub async fn run(_reset_marker: bool) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_input_parses_minimal() {
        let json = br#"{"session_id":"abc123","cwd":"/tmp/repo"}"#;
        let input = HookInput::parse(&json[..]).unwrap();
        assert_eq!(input.session_id, "abc123");
        assert_eq!(input.cwd, "/tmp/repo");
        assert!(input.hook_event_name.is_none());
    }

    #[test]
    fn hook_input_parses_full() {
        let json = br#"{
            "session_id":"sess",
            "cwd":"/tmp",
            "hook_event_name":"UserPromptSubmit",
            "transcript_path":"/tmp/t.jsonl",
            "permission_mode":"default"
        }"#;
        let input = HookInput::parse(&json[..]).unwrap();
        assert_eq!(input.hook_event_name.as_deref(), Some("UserPromptSubmit"));
        assert_eq!(input.transcript_path.as_deref(), Some("/tmp/t.jsonl"));
    }

    #[test]
    fn hook_input_missing_required_field_errors() {
        let json = br#"{"cwd":"/tmp"}"#; // missing session_id
        assert!(HookInput::parse(&json[..]).is_err());
    }

    #[test]
    fn marker_dir_uses_xdg_runtime_dir_when_set() {
        let saved = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", "/tmp/test-xdg");
        let dir = marker_dir();
        assert_eq!(dir, PathBuf::from("/tmp/test-xdg/claude-helix"));
        match saved {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }

    #[test]
    fn marker_path_embeds_session_id() {
        let saved = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::set_var("XDG_RUNTIME_DIR", "/tmp/x");
        let p = marker_path("abc-123");
        assert!(p.to_string_lossy().ends_with("marker-abc-123"));
        match saved {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }

    #[test]
    fn marker_read_write_round_trip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("marker-sess");
        assert!(read_marker_mtime(&path).is_none());
        write_marker_mtime(&path, 1_234_567).unwrap();
        assert_eq!(read_marker_mtime(&path), Some(1_234_567));
    }

    #[test]
    fn marker_read_returns_none_when_corrupted() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("marker-broken");
        std::fs::write(&path, "not a number\n").unwrap();
        assert!(read_marker_mtime(&path).is_none());
    }
}
