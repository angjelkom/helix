# Phase 1 — Schema Extraction + `is_cwd_fallback` Bug Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract context-snapshot types into a shared `helix-context-schema` workspace crate, bump the JSON schema from v1 to v2 (adding `last_update_source`), fix the existing bug that pollutes `$HOME/.helix/` when Helix is launched outside a workspace, and update the Claude Code hook to honor the new source field.

**Architecture:** New workspace member `helix-context-schema` holds pure serde data types — no Helix dependencies. `helix-term::context_logger` is refactored to construct and serialize a `ContextSnapshot` from the shared crate rather than building inline `serde_json::json!`. The shared crate is the single source of truth for the snapshot schema; later phases (the MCP bridge binary) will depend on it without pulling in `helix-term`.

**Tech Stack:** Rust 2021, serde + serde_json (already in workspace deps), chrono (already in `helix-term`). No new external dependencies.

**Spec:** `docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md`, Phase 1 (§11) + the schema definition in §4.

---

## Project context for the implementer

You are working in a Helix editor fork on branch `nightly`. Helix is a modal terminal editor written in Rust, organized as a Cargo workspace with crates like `helix-core`, `helix-view`, `helix-term`, etc.

This fork has a feature called `context_logger` (already shipped) that writes a JSON snapshot of editor state to `<workspace>/.helix/context.json` on terminal focus loss. A Claude Code hook reads that file. This phase extends and refactors that feature without changing its user-visible behavior, except in one case (the bug fix — see Task 6).

The codebase already builds: `cargo check --workspace` is green at the start of this work. `cargo test -p helix-vcs` is broken for unrelated reasons (a feature-flag issue with the `git` module). Use `cargo test -p <crate>` for the specific crates you're touching; do not run workspace-wide tests.

## File structure

**Create (new files):**

- `helix-context-schema/Cargo.toml` — manifest for the new crate. Deps: `serde`, `serde_json`, `chrono` (with `serde` feature for RFC3339 timestamp serialization).
- `helix-context-schema/src/lib.rs` — crate root. Re-exports `ContextSnapshot`, `UpdateSource`, `Active`, `Cursor`, `Selection`, `Position`, `Instance`, `OpenBuffer`. Includes the `SCHEMA_VERSION` const.
- `helix-context-schema/tests/serde_roundtrip.rs` — integration tests verifying JSON round-trip for the schema.

**Modify (existing files):**

- `Cargo.toml` (workspace root) — add `helix-context-schema` to `members`.
- `helix-term/Cargo.toml` — add path-dep on `helix-context-schema`.
- `helix-term/src/context_logger.rs` — refactor `build_snapshot` to build a `ContextSnapshot` value from the new crate; extend `write_context_file` signature to take an `UpdateSource`; add the early-return for `is_cwd_fallback=true`.
- `helix-term/src/ui/editor.rs` — update the single call site at line 1700 (after the dispatch) to pass `UpdateSource::FocusLost`.
- `~/.claude/hooks/helix-context.sh` — add a check that exits without injecting if the snapshot's `last_update_source == "mcp_command"`.

**No new external crate deps**: everything Phase 1 needs is already in the workspace.

---

## Task 1: Create `helix-context-schema` crate skeleton

**Files:**
- Create: `helix-context-schema/Cargo.toml`
- Create: `helix-context-schema/src/lib.rs`
- Modify: `Cargo.toml` (workspace root, add to `members` array)

- [ ] **Step 1: Create `helix-context-schema/Cargo.toml`**

```toml
[package]
name = "helix-context-schema"
description = "JSON snapshot schema shared between Helix and external context-aware tooling."
version = "25.7.1"
authors = ["Helix fork contributors"]
edition = "2021"
license = "MPL-2.0"
homepage = "https://helix-editor.com"
include = ["src/**/*"]

[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

**Note for the implementer:** the workspace `Cargo.toml` does *not* define `serde` in `[workspace.dependencies]` (verified by inspection at task-writing time), so we declare the version directly rather than using `workspace = true`. This matches the pattern used in `helix-term/Cargo.toml`. `chrono` is intentionally absent — the schema stores `timestamp` as a `String`; the producer (`helix-term`) already depends on `chrono` for formatting.

- [ ] **Step 2: Create `helix-context-schema/src/lib.rs` with a minimal `SCHEMA_VERSION` const**

```rust
//! JSON schema for the Helix context snapshot file (`<workspace>/.helix/context.json`).
//!
//! Used by Helix itself (`helix-term::context_logger`) and by external tools that
//! consume the file (e.g. the planned `helix-claude-mcp` bridge). Schema changes
//! happen here once and surface as compile errors on both producers and consumers.

pub const SCHEMA_VERSION: u32 = 2;
pub const MIN_SUPPORTED_READER: u32 = 1;
```

- [ ] **Step 3: Add the crate to the workspace `members` list**

Edit `Cargo.toml` (workspace root). Locate the `[workspace]` block (top of file). Add `"helix-context-schema"` to the `members` array, placed after `"helix-stdx"` to keep alphabetical-ish ordering.

After edit, the `members` list should contain `"helix-context-schema"` as one entry. Do not add it to `default-members`.

- [ ] **Step 4: Verify the workspace still builds**

Run: `cargo check -p helix-context-schema`
Expected: Compiles cleanly (the crate has no real code yet, just the consts).

Run: `cargo check --workspace`
Expected: Compiles cleanly.

- [ ] **Step 5: Commit**

```bash
git add helix-context-schema/Cargo.toml helix-context-schema/src/lib.rs Cargo.toml
git commit -m "$(cat <<'EOF'
feat(context-schema): add helix-context-schema crate skeleton

New workspace member that will hold the JSON snapshot types shared between
Helix and external context-aware tooling. Empty for now — types follow in
subsequent commits.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (Phase 1)
EOF
)"
```

---

## Task 2: Define `UpdateSource` enum

**Files:**
- Create: `helix-context-schema/src/source.rs`
- Modify: `helix-context-schema/src/lib.rs` (add `mod source;` and re-export)
- Create: `helix-context-schema/tests/serde_roundtrip.rs` (first test)

- [ ] **Step 1: Write the failing test**

Create `helix-context-schema/tests/serde_roundtrip.rs`:

```rust
use helix_context_schema::UpdateSource;

#[test]
fn update_source_serializes_to_snake_case() {
    assert_eq!(
        serde_json::to_string(&UpdateSource::FocusLost).unwrap(),
        "\"focus_lost\""
    );
    assert_eq!(
        serde_json::to_string(&UpdateSource::McpCommand).unwrap(),
        "\"mcp_command\""
    );
    assert_eq!(
        serde_json::to_string(&UpdateSource::Manual).unwrap(),
        "\"manual\""
    );
}

#[test]
fn update_source_deserializes_from_snake_case() {
    let parsed: UpdateSource = serde_json::from_str("\"focus_lost\"").unwrap();
    assert!(matches!(parsed, UpdateSource::FocusLost));
}
```

- [ ] **Step 2: Run the test — confirm it fails**

Run: `cargo test -p helix-context-schema`
Expected: FAIL — `helix_context_schema::UpdateSource` does not exist.

- [ ] **Step 3: Implement `UpdateSource`**

Create `helix-context-schema/src/source.rs`:

```rust
use serde::{Deserialize, Serialize};

/// What caused the most recent snapshot write.
///
/// - `FocusLost`: the user switched away from the terminal (default writer).
/// - `McpCommand`: an external tool (the MCP bridge) mutated editor state.
///   Hook readers should treat this as "Claude already knows" and skip injection.
/// - `Manual`: the user explicitly ran the `:write-context` typable command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateSource {
    FocusLost,
    McpCommand,
    Manual,
}
```

- [ ] **Step 4: Wire it into `lib.rs`**

Edit `helix-context-schema/src/lib.rs`:

```rust
//! JSON schema for the Helix context snapshot file (`<workspace>/.helix/context.json`).
//!
//! Used by Helix itself (`helix-term::context_logger`) and by external tools that
//! consume the file (e.g. the planned `helix-claude-mcp` bridge). Schema changes
//! happen here once and surface as compile errors on both producers and consumers.

mod source;

pub use source::UpdateSource;

pub const SCHEMA_VERSION: u32 = 2;
pub const MIN_SUPPORTED_READER: u32 = 1;
```

- [ ] **Step 5: Run the tests — confirm they pass**

Run: `cargo test -p helix-context-schema`
Expected: PASS, 2 tests passing.

- [ ] **Step 6: Commit**

```bash
git add helix-context-schema/src/source.rs helix-context-schema/src/lib.rs helix-context-schema/tests/serde_roundtrip.rs
git commit -m "$(cat <<'EOF'
feat(context-schema): add UpdateSource enum

Tags every snapshot write with its cause: focus_lost (user switched panes),
mcp_command (Claude's MCP bridge mutated state), or manual (explicit
:write-context command). Hook scripts will use this to skip redundant
injections.
EOF
)"
```

---

## Task 3: Define `Position`, `Cursor`, `Selection`, `OpenBuffer`, `Active` types

**Files:**
- Create: `helix-context-schema/src/types.rs`
- Modify: `helix-context-schema/src/lib.rs`
- Modify: `helix-context-schema/tests/serde_roundtrip.rs` (add tests for new types)

- [ ] **Step 1: Add failing tests for the new types**

Append to `helix-context-schema/tests/serde_roundtrip.rs`:

```rust
use helix_context_schema::{Active, Cursor, OpenBuffer, Position, Selection};

#[test]
fn position_serializes_as_object() {
    let p = Position { line: 17, column: 5 };
    let s = serde_json::to_string(&p).unwrap();
    assert_eq!(s, r#"{"line":17,"column":5}"#);
}

#[test]
fn cursor_serializes_with_primary_flag() {
    let c = Cursor { primary: true, line: 17, column: 5 };
    let s = serde_json::to_string(&c).unwrap();
    assert_eq!(s, r#"{"primary":true,"line":17,"column":5}"#);
}

#[test]
fn selection_optional_text_omitted_when_none() {
    let sel = Selection {
        primary: true,
        start: Position { line: 1, column: 1 },
        end: Position { line: 2, column: 3 },
        byte_len: 17,
        text: None,
    };
    let s = serde_json::to_string(&sel).unwrap();
    assert!(!s.contains("\"text\""), "text field should be omitted: {}", s);
}

#[test]
fn active_round_trips_through_serde() {
    let a = Active {
        path: Some("src/main.rs".into()),
        path_abs: Some("/repo/src/main.rs".into()),
        language: Some("rust".into()),
        modified: false,
        line_count: 200,
        cursors: vec![Cursor { primary: true, line: 1, column: 1 }],
        selections: vec![],
        text: None,
    };
    let j = serde_json::to_value(&a).unwrap();
    let back: Active = serde_json::from_value(j).unwrap();
    assert_eq!(a.line_count, back.line_count);
    assert_eq!(a.cursors.len(), back.cursors.len());
}

#[test]
fn open_buffer_round_trips() {
    let b = OpenBuffer {
        path: Some("src/lib.rs".into()),
        language: Some("rust".into()),
        modified: true,
    };
    let j = serde_json::to_value(&b).unwrap();
    let back: OpenBuffer = serde_json::from_value(j).unwrap();
    assert_eq!(b.modified, back.modified);
}
```

- [ ] **Step 2: Run the tests — confirm they fail**

Run: `cargo test -p helix-context-schema`
Expected: FAIL — types not found.

- [ ] **Step 3: Implement the types**

Create `helix-context-schema/src/types.rs`:

```rust
use serde::{Deserialize, Serialize};

/// 1-indexed line/column position. Lines and columns are 1-indexed by convention
/// to match user-visible display in Helix's statusline; internally Helix uses
/// 0-indexed positions, so producers must add 1 before serializing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// A single cursor position. Snapshots include every cursor in a multi-cursor
/// selection, with one marked `primary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    pub primary: bool,
    pub line: usize,
    pub column: usize,
}

/// A visual selection range (only emitted when the user has selected more
/// than one character — single-cursor zero-width ranges are excluded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub primary: bool,
    pub start: Position,
    pub end: Position,
    pub byte_len: usize,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
}

/// Metadata about each open buffer in Helix (not just the active one).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenBuffer {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<String>,
    pub modified: bool,
}

/// State of the currently focused buffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Active {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub path_abs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<String>,
    pub modified: bool,
    pub line_count: usize,
    pub cursors: Vec<Cursor>,
    pub selections: Vec<Selection>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

Edit `helix-context-schema/src/lib.rs`:

```rust
//! JSON schema for the Helix context snapshot file (`<workspace>/.helix/context.json`).
//!
//! Used by Helix itself (`helix-term::context_logger`) and by external tools that
//! consume the file (e.g. the planned `helix-claude-mcp` bridge). Schema changes
//! happen here once and surface as compile errors on both producers and consumers.

mod source;
mod types;

pub use source::UpdateSource;
pub use types::{Active, Cursor, OpenBuffer, Position, Selection};

pub const SCHEMA_VERSION: u32 = 2;
pub const MIN_SUPPORTED_READER: u32 = 1;
```

- [ ] **Step 5: Run the tests — confirm they pass**

Run: `cargo test -p helix-context-schema`
Expected: PASS, 7 tests passing.

- [ ] **Step 6: Commit**

```bash
git add helix-context-schema/src/types.rs helix-context-schema/src/lib.rs helix-context-schema/tests/serde_roundtrip.rs
git commit -m "$(cat <<'EOF'
feat(context-schema): add Position, Cursor, Selection, OpenBuffer, Active types

These mirror the inline JSON the helix-term context_logger currently
produces, with serde's skip_serializing_if to omit None fields exactly as
the existing producer does (preserving wire compatibility with the v1
schema).
EOF
)"
```

---

## Task 4: Define `Instance` (optional snapshot.instance hint block)

**Files:**
- Modify: `helix-context-schema/src/types.rs`
- Modify: `helix-context-schema/src/lib.rs`
- Modify: `helix-context-schema/tests/serde_roundtrip.rs`

- [ ] **Step 1: Write the failing test**

Append to `helix-context-schema/tests/serde_roundtrip.rs`:

```rust
use helix_context_schema::Instance;

#[test]
fn instance_round_trips() {
    let i = Instance {
        pid: 12345,
        socket_path: "/repo/.helix/control-12345.sock".into(),
        started_at: "2026-05-12T10:00:00Z".into(),
    };
    let j = serde_json::to_value(&i).unwrap();
    let back: Instance = serde_json::from_value(j).unwrap();
    assert_eq!(i.pid, back.pid);
    assert_eq!(i.socket_path, back.socket_path);
}
```

- [ ] **Step 2: Run the test — confirm it fails**

Run: `cargo test -p helix-context-schema`
Expected: FAIL — `Instance` not found.

- [ ] **Step 3: Implement `Instance`**

Append to `helix-context-schema/src/types.rs`:

```rust
/// Optional metadata about the running Helix instance that wrote this snapshot.
/// Included by Phase 2 once the control socket is implemented; omitted entirely
/// during Phase 1.
///
/// The socket_path is a *hint* for clients — discovery does not depend on it
/// (clients glob `<workspace>/.helix/control-*.sock` directly).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub pid: u32,
    pub socket_path: String,
    pub started_at: String,
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

Edit `helix-context-schema/src/lib.rs` to add `Instance` to the `pub use types::{...}` list:

```rust
pub use types::{Active, Cursor, Instance, OpenBuffer, Position, Selection};
```

- [ ] **Step 5: Run the tests — confirm they pass**

Run: `cargo test -p helix-context-schema`
Expected: PASS, 8 tests passing.

- [ ] **Step 6: Commit**

```bash
git add helix-context-schema/src/types.rs helix-context-schema/src/lib.rs helix-context-schema/tests/serde_roundtrip.rs
git commit -m "feat(context-schema): add optional Instance hint type for Phase 2 use"
```

---

## Task 5: Define the top-level `ContextSnapshot` type

**Files:**
- Create: `helix-context-schema/src/snapshot.rs`
- Modify: `helix-context-schema/src/lib.rs`
- Modify: `helix-context-schema/tests/serde_roundtrip.rs`

- [ ] **Step 1: Write the failing test**

Append to `helix-context-schema/tests/serde_roundtrip.rs`:

```rust
use helix_context_schema::{ContextSnapshot, SCHEMA_VERSION};

#[test]
fn snapshot_schema_version_is_2() {
    assert_eq!(SCHEMA_VERSION, 2);
}

#[test]
fn snapshot_full_round_trip() {
    let snap = ContextSnapshot {
        schema_version: SCHEMA_VERSION,
        min_supported_reader: 1,
        timestamp: "2026-05-12T14:32:01Z".into(),
        last_update_source: UpdateSource::FocusLost,
        instance: None,
        project_root: "/repo".into(),
        mode: "normal".into(),
        active: Active {
            path: Some("src/main.rs".into()),
            path_abs: Some("/repo/src/main.rs".into()),
            language: Some("rust".into()),
            modified: false,
            line_count: 200,
            cursors: vec![Cursor { primary: true, line: 17, column: 5 }],
            selections: vec![],
            text: None,
        },
        open_buffers: vec![],
    };
    let j = serde_json::to_value(&snap).unwrap();

    assert_eq!(j["schema_version"], 2);
    assert_eq!(j["last_update_source"], "focus_lost");
    assert!(j.get("instance").is_none() || j["instance"].is_null());

    let back: ContextSnapshot = serde_json::from_value(j).unwrap();
    assert_eq!(back.schema_version, 2);
    assert_eq!(back.project_root, "/repo");
}

#[test]
fn snapshot_with_instance_round_trips() {
    let snap = ContextSnapshot {
        schema_version: SCHEMA_VERSION,
        min_supported_reader: 1,
        timestamp: "2026-05-12T14:32:01Z".into(),
        last_update_source: UpdateSource::McpCommand,
        instance: Some(Instance {
            pid: 12345,
            socket_path: "/repo/.helix/control-12345.sock".into(),
            started_at: "2026-05-12T10:00:00Z".into(),
        }),
        project_root: "/repo".into(),
        mode: "normal".into(),
        active: Active {
            path: None, path_abs: None, language: None, modified: false,
            line_count: 0, cursors: vec![], selections: vec![], text: None,
        },
        open_buffers: vec![],
    };
    let j = serde_json::to_value(&snap).unwrap();
    assert_eq!(j["instance"]["pid"], 12345);
    let back: ContextSnapshot = serde_json::from_value(j).unwrap();
    assert!(back.instance.is_some());
}
```

- [ ] **Step 2: Run the tests — confirm they fail**

Run: `cargo test -p helix-context-schema`
Expected: FAIL — `ContextSnapshot` not found.

- [ ] **Step 3: Implement `ContextSnapshot`**

Create `helix-context-schema/src/snapshot.rs`:

```rust
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
```

- [ ] **Step 4: Re-export from `lib.rs`**

Edit `helix-context-schema/src/lib.rs`:

```rust
//! JSON schema for the Helix context snapshot file (`<workspace>/.helix/context.json`).
//!
//! Used by Helix itself (`helix-term::context_logger`) and by external tools that
//! consume the file (e.g. the planned `helix-claude-mcp` bridge). Schema changes
//! happen here once and surface as compile errors on both producers and consumers.

mod snapshot;
mod source;
mod types;

pub use snapshot::ContextSnapshot;
pub use source::UpdateSource;
pub use types::{Active, Cursor, Instance, OpenBuffer, Position, Selection};

pub const SCHEMA_VERSION: u32 = 2;
pub const MIN_SUPPORTED_READER: u32 = 1;
```

- [ ] **Step 5: Run the tests — confirm they pass**

Run: `cargo test -p helix-context-schema`
Expected: PASS, 11 tests passing.

- [ ] **Step 6: Commit**

```bash
git add helix-context-schema/src/snapshot.rs helix-context-schema/src/lib.rs helix-context-schema/tests/serde_roundtrip.rs
git commit -m "$(cat <<'EOF'
feat(context-schema): add top-level ContextSnapshot type

Captures the full v2 schema: schema_version, last_update_source, optional
instance hint block, project_root, mode, active buffer, and open buffers.
Round-trip-tested both with and without the optional instance block.
EOF
)"
```

---

## Task 6: Wire `helix-context-schema` into `helix-term` and refactor `context_logger.rs`

**Files:**
- Modify: `helix-term/Cargo.toml`
- Modify: `helix-term/src/context_logger.rs`

- [ ] **Step 1: Add the dep to `helix-term/Cargo.toml`**

Edit `helix-term/Cargo.toml`. Find the `[dependencies]` section (around line 30+). Add:

```toml
helix-context-schema = { path = "../helix-context-schema" }
```

Place it alphabetically near other `helix-*` deps. Save.

- [ ] **Step 2: Verify the dep is wired**

Run: `cargo check -p helix-term`
Expected: Compiles cleanly. No warnings about unused dep yet (it's not imported).

- [ ] **Step 3: Refactor `context_logger.rs` to use the shared types**

Replace the entire content of `helix-term/src/context_logger.rs` with:

```rust
//! Writes a JSON snapshot of editor state to disk whenever the terminal
//! loses focus (or, in later phases, when triggered by the MCP bridge).
//! Lets external tools read the user's current project, file, cursor, and
//! selection without the user having to copy and paste.
//!
//! Schema lives in the `helix-context-schema` workspace crate.

use std::io::Write;
use std::path::{Path, PathBuf};

use helix_context_schema::{
    Active, ContextSnapshot, Cursor, OpenBuffer, Position, Selection, UpdateSource,
    MIN_SUPPORTED_READER, SCHEMA_VERSION,
};
use helix_core::coords_at_pos;
use helix_view::current_ref;
use helix_view::editor::ContextLoggerConfig;
use helix_view::Editor;

pub fn write_context_file(editor: &Editor, source: UpdateSource) -> std::io::Result<()> {
    let cfg = editor.config().context_logger.clone();
    if !cfg.enabled {
        return Ok(());
    }

    let (workspace, is_cwd_fallback) = helix_loader::find_workspace();
    if is_cwd_fallback {
        log::debug!(
            "context_logger: launched outside a workspace marker — skipping snapshot write \
             (would otherwise pollute {}/.helix/)",
            workspace.display()
        );
        return Ok(());
    }

    let target: PathBuf = if cfg.path.is_absolute() {
        cfg.path.clone()
    } else {
        workspace.join(&cfg.path)
    };

    let snapshot = build_snapshot(editor, &workspace, &cfg, source);
    let payload = serde_json::to_vec_pretty(&snapshot)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut tmp = target.clone();
    let tmp_name = match target.file_name() {
        Some(n) => {
            let mut s = n.to_os_string();
            s.push(".tmp");
            s
        }
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "context_logger path has no filename",
            ))
        }
    };
    tmp.set_file_name(tmp_name);

    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&payload)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &target)?;
    Ok(())
}

fn build_snapshot(
    editor: &Editor,
    workspace: &Path,
    cfg: &ContextLoggerConfig,
    source: UpdateSource,
) -> ContextSnapshot {
    let (view, doc) = current_ref!(editor);
    let text = doc.text();
    let slice = text.slice(..);
    let selection = doc.selection(view.id);
    let primary_idx = selection.primary_index();

    let mut cursors: Vec<Cursor> = Vec::new();
    let mut selections: Vec<Selection> = Vec::new();
    for (i, range) in selection.ranges().iter().enumerate() {
        let cursor_char = range.cursor(slice);
        let cursor_pos = coords_at_pos(slice, cursor_char);
        cursors.push(Cursor {
            primary: i == primary_idx,
            line: cursor_pos.row + 1,
            column: cursor_pos.col + 1,
        });

        let from = range.from();
        let to = range.to();
        if to.saturating_sub(from) > 1 {
            let start = coords_at_pos(slice, from);
            let end = coords_at_pos(slice, to);
            let byte_len = slice.slice(from..to).len_bytes();
            let text_field = if cfg.include_selection_text {
                let raw = slice.slice(from..to).to_string();
                let truncated = if raw.len() > cfg.max_selection_bytes {
                    let mut s: String =
                        raw.chars().take(cfg.max_selection_bytes).collect();
                    s.push_str("\n…[truncated by context_logger]");
                    s
                } else {
                    raw
                };
                Some(truncated)
            } else {
                None
            };
            selections.push(Selection {
                primary: i == primary_idx,
                start: Position {
                    line: start.row + 1,
                    column: start.col + 1,
                },
                end: Position {
                    line: end.row + 1,
                    column: end.col + 1,
                },
                byte_len,
                text: text_field,
            });
        }
    }

    let path_abs: Option<PathBuf> = doc.path().cloned();
    let path_rel: Option<String> = path_abs.as_ref().and_then(|p| {
        p.strip_prefix(workspace)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    });

    let active = Active {
        path: path_rel,
        path_abs: path_abs.as_ref().map(|p| p.to_string_lossy().into_owned()),
        language: doc.language_name().map(|s| s.to_owned()),
        modified: doc.is_modified(),
        line_count: text.len_lines(),
        cursors,
        selections,
        text: if cfg.include_buffer_text {
            Some(text.to_string())
        } else {
            None
        },
    };

    let open_buffers: Vec<OpenBuffer> = editor
        .documents()
        .map(|d| OpenBuffer {
            path: d.path().map(|p| p.to_string_lossy().into_owned()),
            language: d.language_name().map(|s| s.to_owned()),
            modified: d.is_modified(),
        })
        .collect();

    ContextSnapshot {
        schema_version: SCHEMA_VERSION,
        min_supported_reader: MIN_SUPPORTED_READER,
        timestamp: chrono::Utc::now().to_rfc3339(),
        last_update_source: source,
        instance: None,
        project_root: workspace.to_string_lossy().into_owned(),
        mode: editor.mode.to_string(),
        active,
        open_buffers,
    }
}
```

- [ ] **Step 4: Verify it builds**

Run: `cargo check -p helix-term`
Expected: One error pointing at the call site in `ui/editor.rs` because `write_context_file` now takes two args. Task 7 fixes this.

- [ ] **Step 5: Do not commit yet** — the workspace doesn't compile until Task 7. Move on.

---

## Task 7: Update the `write_context_file` call site

**Files:**
- Modify: `helix-term/src/ui/editor.rs`

- [ ] **Step 1: Find the call site**

Run: `grep -n "write_context_file" /Users/angm/helix/helix-term/src/ui/editor.rs`
Expected output: one match near line 1700.

- [ ] **Step 2: Update the call to pass `UpdateSource::FocusLost`**

Locate the existing call:

```rust
if context.editor.config().context_logger.enabled {
    if let Err(e) = crate::context_logger::write_context_file(context.editor) {
        log::warn!("context_logger: failed to write snapshot: {}", e);
    }
}
```

Replace with:

```rust
if context.editor.config().context_logger.enabled {
    if let Err(e) = crate::context_logger::write_context_file(
        context.editor,
        helix_context_schema::UpdateSource::FocusLost,
    ) {
        log::warn!("context_logger: failed to write snapshot: {}", e);
    }
}
```

- [ ] **Step 3: Verify the workspace builds**

Run: `cargo check --workspace`
Expected: Clean, no warnings.

- [ ] **Step 4: Run the existing test suite for `helix-context-schema`**

Run: `cargo test -p helix-context-schema`
Expected: PASS, all tests still green.

- [ ] **Step 5: Build the release binary to confirm everything links**

Run: `cargo build --release -p helix-term --bin hx`
Expected: Successful build at `target/release/hx`.

- [ ] **Step 6: Manual smoke test**

This isn't automatable (requires a real TTY and pane focus), but document the steps:
1. `cp target/release/hx /tmp/hx-test`
2. In Kitty: open `/tmp/hx-test` and edit a file in a real git repo
3. Switch panes (focus loss)
4. Run: `cat <repo>/.helix/context.json | jq '.last_update_source, .schema_version'`
Expected output:
```
"focus_lost"
2
```

- [ ] **Step 7: Commit Tasks 6 + 7 together**

Two tasks combine into one commit because the intermediate state doesn't compile.

```bash
git add helix-term/Cargo.toml helix-term/src/context_logger.rs helix-term/src/ui/editor.rs
git commit -m "$(cat <<'EOF'
refactor(context-logger): adopt shared helix-context-schema types

- Replace inline serde_json::json! with typed ContextSnapshot construction
- Extend write_context_file signature with UpdateSource parameter
  (single Phase 1 caller passes UpdateSource::FocusLost; Phase 2 will
  add McpCommand and Manual sources)
- Bumps emitted schema_version from 1 to 2 (adds last_update_source field)
- No behavior change for users who already had context-logger enabled,
  except the new field appears in the JSON output

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (Phase 1)
EOF
)"
```

---

## Task 8: Fix the `is_cwd_fallback` bug (separate commit for the bugfix portion)

**Files:**
- (Already done in Task 6) `helix-term/src/context_logger.rs`

This task documents the bugfix explicitly. The actual code change (early return on `is_cwd_fallback=true`) shipped as part of Task 6's refactor. We commit a release note here so it's traceable in `git log`.

- [ ] **Step 1: Verify the fix is in place**

Run: `grep -n "is_cwd_fallback" /Users/angm/helix/helix-term/src/context_logger.rs`
Expected: Two lines — the destructure and the `if is_cwd_fallback { return Ok(()); }` early return.

- [ ] **Step 2: Document the bug fix**

Create or append to `helix-term/CHANGELOG.md` (if it exists; otherwise create one):

Run: `ls /Users/angm/helix/helix-term/CHANGELOG.md`

If it doesn't exist, skip this step — the bug fix is documented in the prior commit message. Move on.

If it does exist, prepend:

```markdown
## Unreleased

### Fixed

- `context_logger` no longer writes a snapshot when Helix is launched outside
  any workspace marker (no `.git`, `.svn`, `.jj`, or `.helix` in the cwd or
  any ancestor). Previously the snapshot was written to `$HOME/.helix/context.json`
  in that case, which could contaminate Claude Code sessions started from `$HOME`.
```

- [ ] **Step 3: Commit (only if CHANGELOG.md exists; otherwise skip)**

```bash
git add helix-term/CHANGELOG.md
git commit -m "docs(context-logger): note is_cwd_fallback fix in changelog"
```

---

## Task 9: Update the Claude Code shell hook to check `last_update_source`

**Files:**
- Modify: `/Users/angm/.claude/hooks/helix-context.sh`

**Note for the implementer:** this file is *outside the Helix repo* (it's part of the user's Claude Code configuration). The change is not committed to the Helix git repo. Document the manual edit in the task only.

- [ ] **Step 1: Read the current hook**

Run: `cat /Users/angm/.claude/hooks/helix-context.sh`
Expected: A shell script that reads `.helix/context.json` and prints it wrapped in `<helix-editor-context>` tags. Does not currently check `last_update_source`.

- [ ] **Step 2: Update the hook**

Replace the contents of `/Users/angm/.claude/hooks/helix-context.sh` with:

```bash
#!/usr/bin/env bash
# Injects Helix editor state (current file, cursor, selection) into Claude Code's
# context on every prompt. Reads .helix/context.json from the project the user
# is in, written by Helix's context_logger feature when its terminal loses focus
# or when the MCP bridge updates state (Phase 2+).
#
# Wired in ~/.claude/settings.json under hooks.UserPromptSubmit.

set -euo pipefail

project_dir="${CLAUDE_PROJECT_DIR:-$PWD}"
ctx_file="$project_dir/.helix/context.json"

[ -r "$ctx_file" ] || exit 0

# Drop snapshots older than 24h — likely stale from a prior session.
if command -v stat >/dev/null 2>&1; then
    case "$(uname -s)" in
        Darwin*) mtime=$(stat -f %m "$ctx_file" 2>/dev/null || echo 0);;
        *)       mtime=$(stat -c %Y "$ctx_file" 2>/dev/null || echo 0);;
    esac
    now=$(date +%s)
    if [ $((now - mtime)) -gt 86400 ]; then
        exit 0
    fi
fi

# Skip injection if Claude itself caused the last update (via the MCP bridge):
# Claude already knows about the new state from the tool-call response.
# Phase 1: this branch never matches in practice (no MCP bridge yet), but the
# check is forward-compatible with Phase 2+.
if grep -q '"last_update_source": "mcp_command"' "$ctx_file" 2>/dev/null; then
    exit 0
fi

# Emit context wrapped so Claude can identify and trust it.
printf '<helix-editor-context source="%s">\n' "$ctx_file"
cat "$ctx_file"
printf '\n</helix-editor-context>\n'
```

- [ ] **Step 3: Smoke-test the hook**

Construct a fake snapshot with `mcp_command` source and verify the hook skips:

```bash
mkdir -p /tmp/fake-mcp/.helix
cat > /tmp/fake-mcp/.helix/context.json <<'EOF'
{
  "schema_version": 2,
  "last_update_source": "mcp_command",
  "project_root": "/tmp/fake-mcp",
  "mode": "normal",
  "active": {"path": null, "modified": false, "line_count": 0, "cursors": [], "selections": []},
  "open_buffers": []
}
EOF
CLAUDE_PROJECT_DIR=/tmp/fake-mcp /Users/angm/.claude/hooks/helix-context.sh
echo "exit=$?"
```

Expected: no output (script skips), `exit=0`.

Then test the happy path with a `focus_lost` source:

```bash
sed -i.bak 's/"mcp_command"/"focus_lost"/' /tmp/fake-mcp/.helix/context.json
CLAUDE_PROJECT_DIR=/tmp/fake-mcp /Users/angm/.claude/hooks/helix-context.sh
echo "exit=$?"
```

Expected: full snapshot wrapped in `<helix-editor-context>` tags, `exit=0`.

- [ ] **Step 4: Clean up the smoke-test directory**

```bash
rm -rf /tmp/fake-mcp
```

- [ ] **Step 5: No git commit** — this file is outside the Helix repo. The change is captured in this plan as the canonical record.

---

## Self-review checklist (for the implementer)

After completing all tasks, run these checks before declaring Phase 1 done:

- [ ] `cargo check --workspace` clean
- [ ] `cargo test -p helix-context-schema` passes (11 tests)
- [ ] `cargo build --release -p helix-term --bin hx` succeeds
- [ ] Snapshot written by a release-build `hx` contains `"schema_version": 2` and `"last_update_source": "focus_lost"`
- [ ] Hook script handles missing snapshot file (exits 0 silently)
- [ ] Hook script skips on `mcp_command` source
- [ ] Hook script emits snapshot on `focus_lost` source
- [ ] Running `hx` in a directory with no `.git`/`.svn`/`.jj`/`.helix` marker does **not** create `$HOME/.helix/context.json` (the bug is fixed)
- [ ] `git log --oneline -10` shows the Phase 1 commits with clear messages

## What's NOT in this phase

Avoid scope creep. These belong to later phases:

- **Unix socket server** in Helix — Phase 2.
- **Populating `instance` block** in the snapshot — Phase 2 (Helix needs the socket to write a meaningful `socket_path`).
- **MCP bridge binary** — Phase 4.
- **Rewriting the hook in Rust** — Phase 5.
- **LSP-backed MCP methods** — Phase 3.

If you find yourself touching code outside the scope above, stop and re-read the spec.
