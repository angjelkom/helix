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
    #[allow(dead_code)] // forward-looking — Phase 6b telemetry/routing
    pub hook_event_name: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // forward-looking — Phase 6b telemetry/routing
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

/// Where to look for the snapshot. Priority order:
/// 1. $CLAUDE_PROJECT_DIR/.helix/context.json
/// 2. {input.cwd}/.helix/context.json
/// 3. Walk up from {input.cwd} until a .helix/context.json is found, or root.
pub fn locate_snapshot(input: &HookInput) -> Option<PathBuf> {
    let try_path = |base: &Path| {
        let candidate = base.join(".helix").join("context.json");
        if candidate.is_file() {
            Some(candidate)
        } else {
            None
        }
    };

    if let Some(env_dir) = std::env::var_os("CLAUDE_PROJECT_DIR") {
        if let Some(p) = try_path(Path::new(&env_dir)) {
            return Some(p);
        }
    }

    let cwd = Path::new(&input.cwd);
    if let Some(p) = try_path(cwd) {
        return Some(p);
    }
    for ancestor in cwd.ancestors().skip(1) {
        if let Some(p) = try_path(ancestor) {
            return Some(p);
        }
    }
    None
}

#[derive(Debug, PartialEq)]
pub enum HookDecision {
    /// Skip emission; reason is for logging/debug only.
    Skip(&'static str),
    /// Emit the snapshot at `snapshot_path`, then write `snapshot_mtime`
    /// into the session's marker file.
    Emit {
        snapshot_path: PathBuf,
        snapshot_mtime: u64,
    },
}

const STALE_THRESHOLD_SECS: u64 = 86400; // 24h

/// Inspect the snapshot file's content and mtime against the marker, decide
/// what to do. Pure function once the file is read; easy to test.
pub fn decide(input: &HookInput) -> HookDecision {
    let Some(snapshot_path) = locate_snapshot(input) else {
        return HookDecision::Skip("snapshot not found");
    };

    let mtime = match std::fs::metadata(&snapshot_path).and_then(|m| {
        m.modified()
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .map_err(|_| io::Error::new(io::ErrorKind::Other, "pre-epoch mtime"))
            })
    }) {
        Ok(m) => m,
        Err(_) => return HookDecision::Skip("could not stat snapshot"),
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now.saturating_sub(mtime) > STALE_THRESHOLD_SECS {
        return HookDecision::Skip("snapshot is stale (> 24h old)");
    }

    // Source check: skip if Claude itself caused the last update.
    let snapshot_text = match std::fs::read_to_string(&snapshot_path) {
        Ok(t) => t,
        Err(_) => return HookDecision::Skip("snapshot unreadable"),
    };
    let snap: helix_context_schema::ContextSnapshot =
        match serde_json::from_str(&snapshot_text) {
            Ok(s) => s,
            Err(_) => return HookDecision::Skip("snapshot is not valid v2 JSON"),
        };

    if snap.last_update_source == helix_context_schema::UpdateSource::McpCommand {
        return HookDecision::Skip("source=mcp_command (Claude already knows)");
    }

    // Marker check: skip if we already injected this mtime in this session.
    let marker_p = marker_path(&input.session_id);
    if let Some(existing) = read_marker_mtime(&marker_p) {
        if existing == mtime {
            return HookDecision::Skip("already injected this mtime for this session");
        }
    }

    HookDecision::Emit {
        snapshot_path,
        snapshot_mtime: mtime,
    }
}

pub async fn run(reset_marker: bool) -> Result<()> {
    // Parse stdin. If parsing fails, exit 0 silently — the hook is best-
    // effort and should never fail the user's prompt.
    let input = match HookInput::parse(io::stdin()) {
        Ok(i) => i,
        Err(e) => {
            log::warn!("hook: stdin parse failed: {}", e);
            return Ok(());
        }
    };

    if reset_marker {
        let marker_p = marker_path(&input.session_id);
        match std::fs::remove_file(&marker_p) {
            Ok(()) => log::debug!("hook: cleared marker {}", marker_p.display()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {} // ok, nothing to clear
            Err(e) => log::warn!("hook: clearing marker failed: {}", e),
        }
        return Ok(());
    }

    match decide(&input) {
        HookDecision::Skip(reason) => {
            log::debug!("hook: skip ({})", reason);
            Ok(())
        }
        HookDecision::Emit { snapshot_path, snapshot_mtime } => {
            emit_wrapped_snapshot(&snapshot_path)?;
            // Update the marker AFTER emission so a write failure here doesn't
            // suppress the actually-needed inject next time.
            let marker_p = marker_path(&input.session_id);
            if let Err(e) = write_marker_mtime(&marker_p, snapshot_mtime) {
                log::warn!("hook: writing marker failed: {}", e);
            }
            Ok(())
        }
    }
}

fn emit_wrapped_snapshot(snapshot_path: &Path) -> Result<()> {
    use std::io::Write;
    let body = std::fs::read_to_string(snapshot_path)?;
    let mut out = io::stdout().lock();
    writeln!(out, "<helix-editor-context source=\"{}\">", snapshot_path.display())?;
    out.write_all(body.as_bytes())?;
    if !body.ends_with('\n') {
        writeln!(out)?;
    }
    writeln!(out, "</helix-editor-context>")?;
    out.flush()?;
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

#[cfg(test)]
mod emit_tests {
    use super::*;

    #[test]
    fn emit_wrapped_snapshot_writes_to_stdout_with_tags() {
        // We can't easily intercept stdout in a unit test. Instead test the
        // body-construction logic by extracting it into a helper. For unit
        // tests we just exercise the file-read path.
        let tmp = tempfile::TempDir::new().unwrap();
        let snap = tmp.path().join("context.json");
        std::fs::write(&snap, r#"{"hello":"world"}"#).unwrap();
        // emit_wrapped_snapshot writes to stdout — we cover its output shape
        // in the integration test (Task 5) which spawns the binary. Here we
        // just confirm the file-read path doesn't error.
        assert!(emit_wrapped_snapshot(&snap).is_ok());
    }
}

#[cfg(test)]
mod decide_tests {
    use super::*;
    use helix_context_schema::UpdateSource;

    fn minimal_snapshot_json(source: UpdateSource) -> String {
        let source_str = match source {
            UpdateSource::FocusLost => "focus_lost",
            UpdateSource::McpCommand => "mcp_command",
            UpdateSource::Manual => "manual",
        };
        serde_json::json!({
            "schema_version": 2,
            "min_supported_reader": 1,
            "timestamp": "2026-05-13T10:00:00Z",
            "last_update_source": source_str,
            "project_root": "/tmp/test",
            "mode": "normal",
            "active": {
                "path": "main.rs",
                "modified": false,
                "line_count": 1,
                "cursors": [],
                "selections": []
            },
            "open_buffers": []
        })
        .to_string()
    }

    fn input_at(cwd: &Path, session: &str) -> HookInput {
        HookInput {
            session_id: session.into(),
            cwd: cwd.to_string_lossy().into_owned(),
            hook_event_name: None,
            transcript_path: None,
        }
    }

    fn isolate_env() -> (Option<std::ffi::OsString>, Option<std::ffi::OsString>) {
        let cpd = std::env::var_os("CLAUDE_PROJECT_DIR");
        let xdg = std::env::var_os("XDG_RUNTIME_DIR");
        std::env::remove_var("CLAUDE_PROJECT_DIR");
        (cpd, xdg)
    }

    fn restore_env((cpd, xdg): (Option<std::ffi::OsString>, Option<std::ffi::OsString>)) {
        match cpd {
            Some(v) => std::env::set_var("CLAUDE_PROJECT_DIR", v),
            None => std::env::remove_var("CLAUDE_PROJECT_DIR"),
        }
        match xdg {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }

    #[test]
    fn decide_skips_when_snapshot_not_found() {
        let _saved = isolate_env();
        let tmp = tempfile::TempDir::new().unwrap();
        let input = input_at(tmp.path(), "s1");
        // Restore env BEFORE calling decide so CLAUDE_PROJECT_DIR doesn't
        // leak in from elsewhere (matters when tests run in parallel).
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());
        let result = decide(&input);
        restore_env(_saved);
        assert!(matches!(result, HookDecision::Skip(_)), "got: {:?}", result);
    }

    #[test]
    fn decide_skips_when_source_is_mcp_command() {
        let saved = isolate_env();
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());

        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        std::fs::write(
            helix.join("context.json"),
            minimal_snapshot_json(UpdateSource::McpCommand),
        )
        .unwrap();

        let input = input_at(tmp.path(), "s2");
        let result = decide(&input);
        restore_env(saved);
        match result {
            HookDecision::Skip(reason) => assert!(reason.contains("mcp_command")),
            other => panic!("expected Skip, got {:?}", other),
        }
    }

    #[test]
    fn decide_emits_when_source_is_focus_lost() {
        let saved = isolate_env();
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());

        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        std::fs::write(
            helix.join("context.json"),
            minimal_snapshot_json(UpdateSource::FocusLost),
        )
        .unwrap();

        let input = input_at(tmp.path(), "s3");
        let result = decide(&input);
        restore_env(saved);
        assert!(matches!(result, HookDecision::Emit { .. }), "got: {:?}", result);
    }

    #[test]
    fn decide_emits_when_source_is_manual() {
        let saved = isolate_env();
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());

        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        std::fs::write(
            helix.join("context.json"),
            minimal_snapshot_json(UpdateSource::Manual),
        )
        .unwrap();

        let input = input_at(tmp.path(), "s-manual");
        let result = decide(&input);
        restore_env(saved);
        assert!(matches!(result, HookDecision::Emit { .. }), "got: {:?}", result);
    }

    #[test]
    fn decide_skips_when_marker_matches_mtime() {
        let saved = isolate_env();
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path());

        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        let snap_path = helix.join("context.json");
        std::fs::write(
            &snap_path,
            minimal_snapshot_json(UpdateSource::FocusLost),
        )
        .unwrap();

        let mtime = snap_path
            .metadata()
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Pre-write the marker with the current mtime.
        let input = input_at(tmp.path(), "s4");
        let marker_p = super::marker_path(&input.session_id);
        super::write_marker_mtime(&marker_p, mtime).unwrap();

        let result = decide(&input);
        restore_env(saved);
        match result {
            HookDecision::Skip(reason) => assert!(reason.contains("already injected")),
            other => panic!("expected Skip(already injected), got {:?}", other),
        }
    }
}
