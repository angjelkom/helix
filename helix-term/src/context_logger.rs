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

/// Returns `Ok(true)` when the snapshot file was actually written,
/// `Ok(false)` when the operation was deliberately skipped (logger
/// disabled or launched outside a workspace marker — both are
/// success cases). Callers that care about whether the file changed
/// (e.g., `:write-context`) should inspect the bool; callers that
/// don't can `let _ = …` or pattern-match Ok.
pub fn write_context_file(
    editor: &Editor,
    source: UpdateSource,
    instance: Option<helix_context_schema::Instance>,
) -> std::io::Result<bool> {
    let cfg = editor.config().context_logger.clone();
    if !cfg.enabled {
        return Ok(false);
    }

    let (workspace, is_cwd_fallback) = helix_loader::find_workspace();
    if is_cwd_fallback {
        log::debug!(
            "context_logger: launched outside a workspace marker — skipping snapshot write \
             (would otherwise pollute {}/.helix/)",
            workspace.display()
        );
        return Ok(false);
    }

    let target: PathBuf = if cfg.path.is_absolute() {
        cfg.path.clone()
    } else {
        workspace.join(&cfg.path)
    };

    let snapshot = build_snapshot(editor, &workspace, &cfg, source, instance);
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
        // Deliberately no fsync. The snapshot is a non-durable cache —
        // readers degrade gracefully when it's missing and the next
        // focus-loss or MCP mutation regenerates it. POSIX rename(2)
        // gives readers atomic all-or-nothing visibility; sync_all
        // would only add value across a power loss, which is exactly
        // the case the snapshot doesn't need to survive. Removing the
        // fsync saves ~0.5–5 ms per write on most filesystems and is
        // dominant cost on a sequence of MCP mutations like
        // open-file → goto-line → select.
    }
    std::fs::rename(&tmp, &target)?;
    Ok(true)
}

pub(crate) fn build_snapshot(
    editor: &Editor,
    workspace: &Path,
    cfg: &ContextLoggerConfig,
    source: UpdateSource,
    instance: Option<helix_context_schema::Instance>,
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
                    let mut end = cfg.max_selection_bytes.min(raw.len());
                    while end > 0 && !raw.is_char_boundary(end) {
                        end -= 1;
                    }
                    let mut s = String::from(&raw[..end]);
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
        instance,
        project_root: workspace.to_string_lossy().into_owned(),
        mode: editor.mode.to_string(),
        active,
        open_buffers,
    }
}
