//! MCP Resource handlers. All three read from `<workspace>/.helix/context.json`
//! — Helix's snapshot file. Tools (Phase 4b) will use the socket for live
//! data; Resources stay on the cheap file-read path.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use helix_context_schema::ContextSnapshot;

#[derive(Debug, Clone, Copy)]
pub enum ResourceKind {
    /// `helix://state/current` — the active buffer's state (path, cursor,
    /// selection, mode).
    Current,
    /// `helix://state/buffers` — the list of open buffers.
    Buffers,
    /// `helix://state/snapshot` — the entire snapshot file.
    Snapshot,
}

impl ResourceKind {
    pub const fn uri(self) -> &'static str {
        match self {
            Self::Current => "helix://state/current",
            Self::Buffers => "helix://state/buffers",
            Self::Snapshot => "helix://state/snapshot",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Current => "helix:state:current",
            Self::Buffers => "helix:state:buffers",
            Self::Snapshot => "helix:state:snapshot",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Current => {
                "The currently focused buffer's path, cursor, selection, language, and editor mode."
            }
            Self::Buffers => {
                "List of all open buffers in the running Helix instance."
            }
            Self::Snapshot => {
                "Full snapshot file: timestamp, project root, instance info, active buffer, open buffers."
            }
        }
    }

    pub fn all() -> impl Iterator<Item = Self> {
        [Self::Current, Self::Buffers, Self::Snapshot].into_iter()
    }
}

/// Read the snapshot file. None when missing (Helix not running or
/// context-logger disabled) — that's a normal state, callers handle it
/// by returning a friendly "no snapshot available" Resource body.
fn load_snapshot(workspace: &Path) -> Option<ContextSnapshot> {
    let path = workspace.join(".helix").join("context.json");
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Resolve `<workspace>` for the resource read. Mirrors discovery: env
/// override > CLAUDE_PROJECT_DIR > current dir.
pub fn resolve_workspace(workspace_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = workspace_override {
        return Ok(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from) {
        return Ok(p);
    }
    std::env::current_dir().context("no CLAUDE_PROJECT_DIR and current_dir unavailable")
}

/// Produce the resource body for the given URI. Returns a JSON string in
/// the appropriate shape for rmcp's resource read response. The MIME type
/// is `application/json` for all three.
pub fn read_resource(kind: ResourceKind, workspace: &Path) -> String {
    let snap = match load_snapshot(workspace) {
        Some(s) => s,
        None => {
            return serde_json::json!({
                "error": "no snapshot available",
                "hint": "Helix isn't running, or [editor.context-logger] enabled = false.",
            })
            .to_string();
        }
    };

    match kind {
        ResourceKind::Current => serde_json::json!({
            "active": snap.active,
            "mode": snap.mode,
            "project_root": snap.project_root,
            "timestamp": snap.timestamp,
            "last_update_source": snap.last_update_source,
        })
        .to_string(),
        ResourceKind::Buffers => serde_json::json!({
            "buffers": snap.open_buffers,
            "timestamp": snap.timestamp,
        })
        .to_string(),
        ResourceKind::Snapshot => serde_json::to_string(&snap)
            .unwrap_or_else(|_| "{}".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_snapshot(workspace: &Path, json: &str) {
        let helix = workspace.join(".helix");
        std::fs::create_dir_all(&helix).unwrap();
        std::fs::write(helix.join("context.json"), json).unwrap();
    }

    fn minimal_snapshot() -> String {
        serde_json::json!({
            "schema_version": 2,
            "min_supported_reader": 1,
            "timestamp": "2026-05-13T10:00:00Z",
            "last_update_source": "focus_lost",
            "project_root": "/tmp/test",
            "mode": "normal",
            "active": {
                "path": "main.rs",
                "path_abs": "/tmp/test/main.rs",
                "language": "rust",
                "modified": false,
                "line_count": 5,
                "cursors": [{"primary": true, "line": 1, "column": 1}],
                "selections": []
            },
            "open_buffers": [
                {"path": "/tmp/test/main.rs", "language": "rust", "modified": false}
            ]
        })
        .to_string()
    }

    #[test]
    fn current_resource_returns_active_block() {
        let tmp = TempDir::new().unwrap();
        write_snapshot(tmp.path(), &minimal_snapshot());
        let body = read_resource(ResourceKind::Current, tmp.path());
        let j: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(j["active"]["path"], "main.rs");
        assert_eq!(j["mode"], "normal");
    }

    #[test]
    fn buffers_resource_returns_open_buffers_list() {
        let tmp = TempDir::new().unwrap();
        write_snapshot(tmp.path(), &minimal_snapshot());
        let body = read_resource(ResourceKind::Buffers, tmp.path());
        let j: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(j["buffers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn snapshot_resource_returns_full_snapshot() {
        let tmp = TempDir::new().unwrap();
        write_snapshot(tmp.path(), &minimal_snapshot());
        let body = read_resource(ResourceKind::Snapshot, tmp.path());
        let j: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(j["schema_version"], 2);
        assert_eq!(j["project_root"], "/tmp/test");
    }

    #[test]
    fn missing_snapshot_returns_friendly_error_body() {
        let tmp = TempDir::new().unwrap();
        // No .helix dir, no snapshot
        let body = read_resource(ResourceKind::Current, tmp.path());
        let j: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(j["error"].is_string());
    }

    #[test]
    fn all_kinds_iterates_three_kinds() {
        let kinds: Vec<_> = ResourceKind::all().collect();
        assert_eq!(kinds.len(), 3);
        let uris: Vec<_> = kinds.iter().map(|k| k.uri()).collect();
        assert!(uris.contains(&"helix://state/current"));
        assert!(uris.contains(&"helix://state/buffers"));
        assert!(uris.contains(&"helix://state/snapshot"));
    }
}
