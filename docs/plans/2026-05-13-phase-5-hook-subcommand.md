# Phase 5 — Rust `hook` Subcommand — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the shell hook (`~/.claude/hooks/helix-context.sh`) with a `helix-claude-mcp hook` subcommand. Same wire contract — read Claude Code's hook payload from stdin, emit a wrapped snapshot to stdout (or nothing) — but in Rust, with proper marker-file dedup, session-keyed state, compression handling, and type-safe parsing.

**Architecture:** A new `hook` module in `helix-claude-mcp` that the existing `Command::Hook` clap variant dispatches to. Three behavioral modes:
1. Normal call (UserPromptSubmit): parse stdin → load snapshot → run dedup checks → emit wrapped snapshot if not skipped → update marker.
2. `--reset-marker` flag (used by PostCompact + SessionStart matcher=compact): delete the session marker file and exit.
3. No snapshot / stale / unreadable: exit 0 silently (the hook is best-effort; never fails the user's prompt).

The marker file lives at `$XDG_RUNTIME_DIR/claude-helix/marker-${session_id}` on Linux, `~/Library/Caches/claude-helix/marker-${session_id}` on macOS, with `~/.cache/claude-helix/` as fallback. The marker's content is the snapshot's mtime (Unix epoch seconds) at last injection.

**Tech Stack:** Same as Phases 4a/4b. Reuses `helix-context-schema::ContextSnapshot` for typed snapshot parsing — no longer parsing JSON ad-hoc like the shell hook.

**Spec:** `docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md` §7.6 (Hook subcommand dedup logic). Phase 4a/b explicitly deferred the hook subcommand to Phase 5.

---

## Project context for the implementer

You are working in a Helix editor fork at `/Users/angm/helix` on branch `nightly`. Phase 4b is complete (tip: `a5d5f8dcf`, 73 commits ahead of remote). `helix-claude-mcp` has a working `serve` subcommand (stdio MCP server with 3 Resources + 7 Tools). The `hook` subcommand currently bails with `anyhow::bail!("hook is a Phase 5 deliverable")`.

The current shell hook (`/Users/angm/.claude/hooks/helix-context.sh`, ~30 lines) does:
- Read `.helix/context.json` from `$CLAUDE_PROJECT_DIR` (or `$PWD` as fallback)
- Skip if file is missing or mtime > 24h ago
- Skip if file contains `"last_update_source": "mcp_command"` (grep-based)
- Otherwise `cat` the file wrapped in `<helix-editor-context>` tags

What Phase 5 adds beyond the shell hook:
- Proper session-keyed mtime marker (the shell hook has no per-session memory — it re-emits the same snapshot every prompt, even if Claude already saw it)
- Typed JSON parsing (no `grep` shenanigans)
- `--reset-marker` for the compression-aware reset path
- Cross-platform marker storage (XDG / Caches / .cache)
- Clear semantics: every skip path is documented and tested

What Phase 5 does NOT do:
- Modify the user's `~/.claude/settings.json` — that's a per-user decision. Task 6 documents the change required.
- Remove the shell hook file — leaving it in place lets the user switch back if needed.

## File structure

**Modify:**

- `helix-claude-mcp/src/main.rs` — extend the `Hook` variant to take `--reset-marker`, dispatch to `hook::run`.
- `helix-claude-mcp/README.md` — section on the hook subcommand and how to wire it.

**Create:**

- `helix-claude-mcp/src/hook.rs` — full hook implementation + unit tests.
- `helix-claude-mcp/tests/hook_integration.rs` — subprocess-driven integration tests (parallel to the `serve` integration tests).

**No new external deps** — only stdlib + existing tokio/serde/serde_json/anyhow.

## Type design

```rust
#[derive(serde::Deserialize)]
pub struct HookInput {
    pub session_id: String,
    pub cwd: String,
    #[serde(default)]
    pub hook_event_name: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    // Other fields exist in the payload but we don't need them; serde ignores
    // unknown fields by default.
}
```

Three platforms-aware helpers (private):

```rust
fn marker_dir() -> PathBuf;          // platform-specific dir
fn marker_path(session_id: &str) -> PathBuf;   // marker_dir().join(format!("marker-{}", session_id))
fn read_marker_mtime(path: &Path) -> Option<u64>;
fn write_marker_mtime(path: &Path, mtime: u64) -> io::Result<()>;
```

One snapshot-discovery helper:

```rust
/// Find <workspace>/.helix/context.json. Tries $CLAUDE_PROJECT_DIR first, then
/// `cwd` from the stdin payload, then walks up from `cwd` looking for a
/// `.helix/` directory.
fn locate_snapshot(input: &HookInput) -> Option<PathBuf>;
```

The decision function:

```rust
#[derive(Debug, PartialEq)]
enum HookDecision {
    Skip(&'static str),     // reason for telemetry / debugging
    Emit { snapshot_path: PathBuf, snapshot_mtime: u64 },
}

fn decide(input: &HookInput) -> HookDecision;
```

Output format unchanged from shell hook:

```
<helix-editor-context source="<path>">
<full snapshot json>
</helix-editor-context>
```

---

## Task 1: Wire `hook` subcommand in clap + main dispatcher

**Files:**
- Modify: `helix-claude-mcp/src/main.rs`

- [ ] **Step 1: Extend the `Hook` variant to take `--reset-marker`**

Find the `Command` enum in `main.rs`. Change:

```rust
    /// Run the UserPromptSubmit hook (Phase 5; not yet implemented).
    Hook,
```

To:

```rust
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
```

- [ ] **Step 2: Add `mod hook;` and dispatch**

Add `mod hook;` near the other module declarations. Change the `Command::Hook` match arm from:

```rust
        Command::Hook => {
            anyhow::bail!("hook is a Phase 5 deliverable");
        }
```

To:

```rust
        Command::Hook { reset_marker } => {
            hook::run(reset_marker).await
        }
```

Note: `hook::run` returns `anyhow::Result<()>`. Tasks 2-5 build out `hook.rs` until it's a real implementation.

- [ ] **Step 3: Create a minimal stub `helix-claude-mcp/src/hook.rs`**

```rust
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
```

- [ ] **Step 4: Verify build**

Run: `cargo check --workspace`
Expected: Clean.

Run: `cargo run -p helix-claude-mcp -- hook --help`
Expected: clap help showing the `--reset-marker` flag.

Run: `cargo run -p helix-claude-mcp -- hook --reset-marker` (with empty stdin)
Expected: exits 0 (the stub does nothing).

- [ ] **Step 5: Commit**

```bash
git add helix-claude-mcp/src/main.rs helix-claude-mcp/src/hook.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): wire hook subcommand clap + dispatch

Hook variant now takes a --reset-marker flag. Main dispatches to
hook::run(reset_marker).

The hook module is a stub (Ok(())) for now — tasks 2-5 build out the
real implementation:
  - parse stdin JSON (Claude Code's hook payload)
  - locate the snapshot file
  - check source + mtime against per-session marker
  - emit wrapped snapshot or skip

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.6)
EOF
)"
```

---

## Task 2: `HookInput` stdin JSON struct + marker path resolution

**Files:**
- Modify: `helix-claude-mcp/src/hook.rs`

- [ ] **Step 1: Write failing tests**

Replace the contents of `helix-claude-mcp/src/hook.rs` with:

```rust
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
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p helix-claude-mcp`
Expected: 27 prior + 7 new = 34 tests pass.

- [ ] **Step 3: Commit**

```bash
git add helix-claude-mcp/src/hook.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): HookInput parsing + marker file path resolution

HookInput is the serde-deserializable subset of Claude Code's hook stdin
payload — session_id and cwd are required, everything else is optional
(serde drops unknown fields).

marker_dir / marker_path / read_marker_mtime / write_marker_mtime
implement cross-platform per-session marker storage. Linux:
$XDG_RUNTIME_DIR/claude-helix/. macOS: ~/Library/Caches/claude-helix/
(or $XDG_RUNTIME_DIR if set). Other: ~/.cache/claude-helix/.

Seven unit tests cover required-field validation, full-payload parsing,
env-var-driven path resolution, write/read round-trip, corrupted-file
handling.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.6)
EOF
)"
```

---

## Task 3: Snapshot location + decision logic

**Files:**
- Modify: `helix-claude-mcp/src/hook.rs`

- [ ] **Step 1: Add the snapshot locator + decision logic with tests**

Add to `helix-claude-mcp/src/hook.rs`. Place the new code after the marker helpers and before `pub async fn run`.

```rust
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
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p helix-claude-mcp`
Expected: 34 prior + 4 new = 38 tests pass. (Tests live in `mod decide_tests` — make sure cargo discovers them; that's normal in Rust.)

If tests are flaky due to env-var races (parallel test execution shares process env), serialize them with `#[serial_test]` from a small new dev-dep — but try running first. The `isolate_env`/`restore_env` pattern should be enough.

- [ ] **Step 3: Commit**

```bash
git add helix-claude-mcp/src/hook.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): snapshot locator + decide() logic

locate_snapshot tries $CLAUDE_PROJECT_DIR first, then input.cwd, then
walks up looking for .helix/context.json.

decide() is a pure-function decision tree:
- snapshot not found → Skip
- snapshot stat fails or pre-epoch → Skip
- snapshot older than 24h → Skip
- snapshot unreadable / bad JSON → Skip
- last_update_source == McpCommand → Skip (Claude knows)
- marker mtime == snapshot mtime → Skip (already injected this session)
- otherwise → Emit { snapshot_path, snapshot_mtime }

Four tests cover: snapshot not found, source=mcp_command, source=
focus_lost happy path, marker-matches dedup.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.6)
EOF
)"
```

---

## Task 4: Full `run()` implementation — emit, update marker, handle `--reset-marker`

**Files:**
- Modify: `helix-claude-mcp/src/hook.rs`

- [ ] **Step 1: Replace `run()` with the full implementation**

Find the `pub async fn run(_reset_marker: bool) -> Result<()>` stub. Replace with:

```rust
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
```

- [ ] **Step 2: Add tests for emission**

Append to the existing `decide_tests` module (or open a new `emit_tests` mod):

```rust
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
```

- [ ] **Step 3: Verify build and run tests**

Run: `cargo check --workspace`
Expected: Clean.

Run: `cargo test -p helix-claude-mcp`
Expected: 38 prior + 1 new = 39 tests pass.

- [ ] **Step 4: Manual smoke test**

```bash
# Build
cargo build --release -p helix-claude-mcp

# Set up a sample snapshot
mkdir -p /tmp/p5-smoke/.helix
cat > /tmp/p5-smoke/.helix/context.json <<'EOF'
{
  "schema_version": 2,
  "min_supported_reader": 1,
  "timestamp": "2026-05-13T10:00:00Z",
  "last_update_source": "focus_lost",
  "project_root": "/tmp/p5-smoke",
  "mode": "normal",
  "active": {"path": "main.rs", "modified": false, "line_count": 1, "cursors": [], "selections": []},
  "open_buffers": []
}
EOF

# Set XDG to a test dir so we don't touch the real marker
mkdir -p /tmp/p5-xdg

# First call: should emit
echo '{"session_id":"smoke","cwd":"/tmp/p5-smoke"}' | \
    XDG_RUNTIME_DIR=/tmp/p5-xdg \
    CLAUDE_PROJECT_DIR=/tmp/p5-smoke \
    /Users/angm/helix/target/release/helix-claude-mcp hook

# Expected output: <helix-editor-context source="..."> ... </helix-editor-context>

# Second call: should skip (marker matches)
echo '{"session_id":"smoke","cwd":"/tmp/p5-smoke"}' | \
    XDG_RUNTIME_DIR=/tmp/p5-xdg \
    CLAUDE_PROJECT_DIR=/tmp/p5-smoke \
    /Users/angm/helix/target/release/helix-claude-mcp hook

# Expected output: (nothing)

# --reset-marker: should silently clear, no output
echo '{"session_id":"smoke","cwd":"/tmp/p5-smoke"}' | \
    XDG_RUNTIME_DIR=/tmp/p5-xdg \
    CLAUDE_PROJECT_DIR=/tmp/p5-smoke \
    /Users/angm/helix/target/release/helix-claude-mcp hook --reset-marker

# Expected output: (nothing)

# Third call: should emit again (marker was cleared)
echo '{"session_id":"smoke","cwd":"/tmp/p5-smoke"}' | \
    XDG_RUNTIME_DIR=/tmp/p5-xdg \
    CLAUDE_PROJECT_DIR=/tmp/p5-smoke \
    /Users/angm/helix/target/release/helix-claude-mcp hook

# Expected output: <helix-editor-context ...> ... </helix-editor-context>

# Cleanup
rm -rf /tmp/p5-smoke /tmp/p5-xdg
```

Expected behaviors:
1. First call emits the wrapped snapshot
2. Second call is silent (marker matches)
3. `--reset-marker` is silent
4. Third call emits again (marker was cleared)

- [ ] **Step 5: Commit**

```bash
git add helix-claude-mcp/src/hook.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): full hook implementation

run() does the real work:
- Parse stdin JSON (HookInput); failed parse → silent exit (hook is
  best-effort, must never fail user's prompt).
- --reset-marker: delete the per-session marker file, return.
- Otherwise: decide() then either Skip (log debug) or Emit (write the
  wrapped snapshot to stdout, then update the marker with the snapshot's
  mtime).

emit_wrapped_snapshot mimics the old shell hook's output:
<helix-editor-context source="..."> { snapshot body } </helix-editor-context>

Marker is written AFTER emission — if the write fails, the next prompt
re-emits rather than missing an inject. Better noisy than silent.

Smoke-tested: first call emits, second skips (marker matches), reset
clears, third emits again.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.6)
EOF
)"
```

---

## Task 5: Subprocess integration tests for the hook subcommand

**Files:**
- Create: `helix-claude-mcp/tests/hook_integration.rs`

- [ ] **Step 1: Write the integration tests**

Create `helix-claude-mcp/tests/hook_integration.rs`:

```rust
//! Integration tests for `helix-claude-mcp hook`. Spawn the binary as
//! Claude Code does — write the stdin payload, capture stdout, check
//! behavior.

use std::process::Stdio;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const SAMPLE_SNAPSHOT_FOCUS_LOST: &str = r#"{
  "schema_version": 2,
  "min_supported_reader": 1,
  "timestamp": "2026-05-13T10:00:00Z",
  "last_update_source": "focus_lost",
  "project_root": "/tmp/p5-test",
  "mode": "normal",
  "active": {"path": "main.rs", "modified": false, "line_count": 1, "cursors": [], "selections": []},
  "open_buffers": []
}"#;

const SAMPLE_SNAPSHOT_MCP_COMMAND: &str = r#"{
  "schema_version": 2,
  "min_supported_reader": 1,
  "timestamp": "2026-05-13T10:00:00Z",
  "last_update_source": "mcp_command",
  "project_root": "/tmp/p5-test",
  "mode": "normal",
  "active": {"path": "main.rs", "modified": false, "line_count": 1, "cursors": [], "selections": []},
  "open_buffers": []
}"#;

fn binary_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_helix-claude-mcp"))
}

async fn run_hook(
    workspace: &std::path::Path,
    xdg: &std::path::Path,
    stdin_payload: &str,
    reset_marker: bool,
) -> (String, String, i32) {
    let mut cmd = Command::new(binary_path());
    cmd.arg("hook");
    if reset_marker {
        cmd.arg("--reset-marker");
    }
    let output = cmd
        .env("CLAUDE_PROJECT_DIR", workspace)
        .env("XDG_RUNTIME_DIR", xdg)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut child = output;
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(stdin_payload.as_bytes()).await.unwrap();
    drop(stdin);

    let output = child.wait_with_output().await.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let code = output.status.code().unwrap_or(-1);
    (stdout, stderr, code)
}

#[tokio::test]
async fn emits_wrapped_snapshot_on_first_call() {
    let workspace = TempDir::new().unwrap();
    let helix = workspace.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT_FOCUS_LOST).unwrap();
    let xdg = TempDir::new().unwrap();

    let payload = r#"{"session_id":"sess-emit","cwd":"PLACEHOLDER"}"#
        .replace("PLACEHOLDER", workspace.path().to_str().unwrap());

    let (stdout, _stderr, code) = run_hook(
        workspace.path(),
        xdg.path(),
        &payload,
        false,
    ).await;
    assert_eq!(code, 0, "non-zero exit");
    assert!(stdout.contains("<helix-editor-context"), "missing opening tag: {}", stdout);
    assert!(stdout.contains("</helix-editor-context>"), "missing closing tag: {}", stdout);
    assert!(stdout.contains("\"last_update_source\""), "missing snapshot body: {}", stdout);
}

#[tokio::test]
async fn skips_silently_on_second_call_with_same_session() {
    let workspace = TempDir::new().unwrap();
    let helix = workspace.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT_FOCUS_LOST).unwrap();
    let xdg = TempDir::new().unwrap();

    let payload = format!(
        r#"{{"session_id":"sess-dup","cwd":"{}"}}"#,
        workspace.path().to_str().unwrap()
    );

    // First call: emit
    let (out1, _, c1) = run_hook(workspace.path(), xdg.path(), &payload, false).await;
    assert_eq!(c1, 0);
    assert!(out1.contains("<helix-editor-context"));

    // Second call: silent
    let (out2, _, c2) = run_hook(workspace.path(), xdg.path(), &payload, false).await;
    assert_eq!(c2, 0);
    assert!(out2.is_empty(), "second call should be silent, got: {:?}", out2);
}

#[tokio::test]
async fn skips_when_source_is_mcp_command() {
    let workspace = TempDir::new().unwrap();
    let helix = workspace.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT_MCP_COMMAND).unwrap();
    let xdg = TempDir::new().unwrap();

    let payload = format!(
        r#"{{"session_id":"sess-mcp","cwd":"{}"}}"#,
        workspace.path().to_str().unwrap()
    );
    let (stdout, _, code) = run_hook(workspace.path(), xdg.path(), &payload, false).await;
    assert_eq!(code, 0);
    assert!(stdout.is_empty(), "should skip on mcp_command source, got: {:?}", stdout);
}

#[tokio::test]
async fn reset_marker_clears_then_next_call_emits() {
    let workspace = TempDir::new().unwrap();
    let helix = workspace.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT_FOCUS_LOST).unwrap();
    let xdg = TempDir::new().unwrap();

    let payload = format!(
        r#"{{"session_id":"sess-reset","cwd":"{}"}}"#,
        workspace.path().to_str().unwrap()
    );

    // First emit creates the marker
    let (out1, _, _) = run_hook(workspace.path(), xdg.path(), &payload, false).await;
    assert!(out1.contains("<helix-editor-context"));
    // Second call is silent (marker matches)
    let (out2, _, _) = run_hook(workspace.path(), xdg.path(), &payload, false).await;
    assert!(out2.is_empty());
    // Reset clears the marker
    let (out3, _, c3) = run_hook(workspace.path(), xdg.path(), &payload, true).await;
    assert_eq!(c3, 0);
    assert!(out3.is_empty());
    // Next call emits again
    let (out4, _, _) = run_hook(workspace.path(), xdg.path(), &payload, false).await;
    assert!(out4.contains("<helix-editor-context"), "post-reset should re-emit, got: {:?}", out4);
}

#[tokio::test]
async fn silent_when_no_snapshot_present() {
    let workspace = TempDir::new().unwrap();
    // No .helix/ dir at all.
    let xdg = TempDir::new().unwrap();

    let payload = format!(
        r#"{{"session_id":"sess-none","cwd":"{}"}}"#,
        workspace.path().to_str().unwrap()
    );
    let (stdout, _, code) = run_hook(workspace.path(), xdg.path(), &payload, false).await;
    assert_eq!(code, 0);
    assert!(stdout.is_empty());
}

#[tokio::test]
async fn silent_on_malformed_stdin() {
    let workspace = TempDir::new().unwrap();
    let xdg = TempDir::new().unwrap();

    let (stdout, _, code) = run_hook(
        workspace.path(),
        xdg.path(),
        "this is not json at all",
        false,
    ).await;
    assert_eq!(code, 0, "must exit 0 even on bad input");
    assert!(stdout.is_empty());
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test -p helix-claude-mcp --test hook_integration`
Expected: 6 tests pass.

- [ ] **Step 3: Run all tests**

Run: `cargo test -p helix-claude-mcp`
Expected: 27 prior + 12 new (unit + integration) = 39 unit tests + 6 + 3 = 45 total? Recount:
- Unit (in src/): 21 (Phase 4b) + 7 (T2) + 4 (T3) + 1 (T4) = 33
- Integration (tests/integration.rs): 6 (Phase 4b)
- Hook integration (tests/hook_integration.rs): 6 (T5)
- Total: 33 + 6 + 6 = 45

Don't worry about exact arithmetic if numbers differ. The point: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add helix-claude-mcp/tests/hook_integration.rs
git commit -m "$(cat <<'EOF'
test(claude-mcp): subprocess integration tests for hook subcommand

Six tests that spawn the binary as Claude Code does, write a stdin
payload, capture stdout, and assert behavior:

1. First call emits the wrapped snapshot
2. Second call with same session is silent (marker dedup)
3. mcp_command source skips (Claude already knows)
4. --reset-marker clears, then next call re-emits
5. No snapshot present → silent exit 0
6. Malformed stdin JSON → silent exit 0 (best-effort hook)

CLAUDE_PROJECT_DIR and XDG_RUNTIME_DIR are isolated per-test using
TempDir — no interference with the user's real marker files.
EOF
)"
```

---

## Task 6: README + Claude Code settings.json instructions

**Files:**
- Modify: `helix-claude-mcp/README.md`

- [ ] **Step 1: Update the README**

Edit `helix-claude-mcp/README.md`. Find the "Subcommands" section. Replace the `hook` bullet (which said Phase 5 not implemented) with a full section. Also add a "Migrating from the shell hook" section.

Add (or replace existing placeholder):

```markdown
## Hook subcommand

`helix-claude-mcp hook` is the Rust replacement for the shell hook script at `~/.claude/hooks/helix-context.sh`. Same wire contract — reads Claude Code's hook payload on stdin, writes the wrapped snapshot to stdout (or nothing if skipped). Use it in two places:

### UserPromptSubmit

Inject the snapshot at the start of every prompt (skipped when already-injected or when the snapshot's `last_update_source: "mcp_command"`):

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          { "type": "command", "command": "helix-claude-mcp hook", "timeout": 5 }
        ]
      }
    ]
  }
}
```

### Compression-aware reset

When Claude Code compacts the context (auto or `/compact`), the previously-injected snapshot is gone. Clear the marker so the next prompt re-injects:

```json
{
  "hooks": {
    "PostCompact": [
      {
        "hooks": [
          { "type": "command", "command": "helix-claude-mcp hook --reset-marker", "timeout": 5 }
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "compact",
        "hooks": [
          { "type": "command", "command": "helix-claude-mcp hook --reset-marker", "timeout": 5 }
        ]
      }
    ]
  }
}
```

### How it dedupes

Marker file at `$XDG_RUNTIME_DIR/claude-helix/marker-${session_id}` (Linux) or `~/Library/Caches/claude-helix/marker-${session_id}` (macOS) holds the snapshot's mtime at last injection. On each call:

1. Parse stdin (must contain `session_id` and `cwd`; serde drops unknown fields).
2. Locate the snapshot at `$CLAUDE_PROJECT_DIR/.helix/context.json` (or walk up from `cwd`).
3. Skip if missing, > 24h stale, malformed, or `last_update_source == "mcp_command"`.
4. Skip if marker mtime matches snapshot mtime (already injected this session).
5. Otherwise: emit wrapped snapshot, then write snapshot mtime into the marker file.

Failure modes (stdin parse error, marker write failure, etc.) exit 0 silently — the hook is best-effort and never fails the user's prompt.

## Migrating from the shell hook

If you previously used the shell hook at `~/.claude/hooks/helix-context.sh`, replace your `~/.claude/settings.json` hooks block. The shell hook can be deleted after switching; nothing references it.

Old:
```json
{ "type": "command", "command": "/Users/you/.claude/hooks/helix-context.sh" }
```

New:
```json
{ "type": "command", "command": "helix-claude-mcp hook", "timeout": 5 }
```

The Rust hook is functionally a superset of the shell version: same emit format, plus proper per-session dedup (the shell version had none — it re-emitted on every prompt) and `--reset-marker` for compression.
```

- [ ] **Step 2: Commit**

```bash
git add helix-claude-mcp/README.md
git commit -m "$(cat <<'EOF'
docs(claude-mcp): hook subcommand documentation

Replaces the Phase 4a placeholder ("Phase 5 — not yet implemented") with
a full hook subcommand section: UserPromptSubmit wiring, compression-aware
--reset-marker wiring, and the dedup logic walkthrough.

Adds a "Migrating from the shell hook" section showing the exact
~/.claude/settings.json change.

Phase 5 complete.
EOF
)"
```

---

## Self-review checklist

After all 6 tasks:

- [ ] `cargo check --workspace` clean
- [ ] `cargo test -p helix-claude-mcp` — at least 45 tests pass (27 from Phase 4 + 12 new unit + 6 new hook integration)
- [ ] `cargo build --release -p helix-claude-mcp` succeeds
- [ ] Smoke test in Task 4 Step 4 ran and all 4 cases produced expected behavior
- [ ] `git log --oneline -8` shows the 6 Phase 5 commits in clean order

## What's NOT in Phase 5

- Automatic modification of `~/.claude/settings.json` — left to the user (per the spec's deliberate stance: per-user decision, not something this fork modifies for you).
- Deletion of the existing shell hook file — leaving it lets users switch back if they hit issues.
- Telemetry / metrics on hook decisions — Phase 6.
- `helix-claude-mcp doctor` for diagnostics — Phase 6.

## Open questions

1. **Test parallelism env races.** The unit tests in Task 3 manipulate process env vars (`XDG_RUNTIME_DIR`, `CLAUDE_PROJECT_DIR`). cargo test runs tests in parallel by default. The plan's `isolate_env`/`restore_env` helper is best-effort. If tests are flaky in CI, two options: serialize via `#[serial_test]` (small dev-dep), or run with `--test-threads=1`. Likely fine on a single workstation, but flag for the implementer.

2. **Marker dir permissions.** The plan creates `marker_dir()` with `0o700` on first write. If `$XDG_RUNTIME_DIR` is a shared dir (rare), the `set_permissions` call may fail — the plan handles this by ignoring the chmod error. The marker file content is just an mtime int; no secret is leaked.

3. **Decision telemetry.** `HookDecision::Skip(reason)` carries a `&'static str` for telemetry. We `log::debug!` it. If a user wants visibility into "why didn't my hook fire", they need to enable debug logging via `RUST_LOG=helix_claude_mcp=debug`. Not a great UX; Phase 6 could add a `--verbose` flag.
