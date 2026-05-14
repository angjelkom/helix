//! `doctor` subcommand. Runs the five checks a freshly-installed bridge
//! has to pass to be useful: binary on PATH, workspace resolvable,
//! snapshot file present and parseable, control socket connectable,
//! `initialize` handshake succeeds. Prints a human-readable report and
//! returns Ok regardless of outcome — diagnostics never fail.

use std::path::{Path, PathBuf};
use std::time::Duration;

use helix_context_schema::{ClientInfo, ContextSnapshot, ControlRequest, ControlResponse};

use crate::{discovery, resources, rpc_client};

/// Aggregated outcome of the five checks. Pure data; rendering is on
/// `Self::render` for ease of unit testing.
pub struct Report {
    pub binary_on_path: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub snapshot: SnapshotCheck,
    pub socket: SocketCheck,
    pub initialize: InitializeCheck,
}

pub enum SnapshotCheck {
    Missing(PathBuf),
    Unreadable(PathBuf, String),
    InvalidJson(PathBuf, String),
    Found {
        path: PathBuf,
        schema_version: u32,
        age_secs: u64,
    },
}

pub enum SocketCheck {
    /// No live socket found under the workspace's `.helix/`.
    None,
    /// Live socket connected during discovery. The path is the resolved
    /// socket file (or, in the pointer-file fallback case, the target the
    /// pointer pointed to).
    Live(PathBuf),
}

pub enum InitializeCheck {
    /// Skipped because there was no live socket to handshake against.
    Skipped,
    /// Helix responded with its protocol_version. The string is the
    /// version Helix advertised.
    Ok(String),
    /// Helix returned an unexpected shape, the transport errored, or the
    /// handshake timed out. The string carries the underlying error.
    Failed(String),
}

impl Report {
    /// Render the report as a human-readable five-line block (plus header).
    /// Format is stable — tools that parse it can match on the leading
    /// label of each line.
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str("helix-mcp doctor\n================\n\n");
        s.push_str(&format!(
            "binary on PATH      : {}\n",
            self.binary_on_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| {
                    "(NOT FOUND — install with `cargo install --path helix-mcp`)".into()
                })
        ));
        s.push_str(&format!(
            "workspace           : {}\n",
            self.workspace
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| {
                    "(unable to resolve — CLAUDE_PROJECT_DIR unset and cwd has no .helix/ ancestor)"
                        .into()
                })
        ));
        s.push_str("snapshot            : ");
        match &self.snapshot {
            SnapshotCheck::Missing(p) => {
                s.push_str(&format!("MISSING at {}\n", p.display()))
            }
            SnapshotCheck::Unreadable(p, e) => {
                s.push_str(&format!("UNREADABLE at {}: {}\n", p.display(), e))
            }
            SnapshotCheck::InvalidJson(p, e) => {
                s.push_str(&format!("INVALID JSON at {}: {}\n", p.display(), e))
            }
            SnapshotCheck::Found {
                path,
                schema_version,
                age_secs,
            } => s.push_str(&format!(
                "OK at {} (schema_version={}, {}s old)\n",
                path.display(),
                schema_version,
                age_secs
            )),
        }
        s.push_str("control socket      : ");
        match &self.socket {
            SocketCheck::None => s.push_str(
                "NOT FOUND (Helix isn't running, or [editor.control-socket] enabled = false)\n",
            ),
            SocketCheck::Live(p) => s.push_str(&format!("LIVE at {}\n", p.display())),
        }
        s.push_str("initialize handshake: ");
        match &self.initialize {
            InitializeCheck::Skipped => s.push_str("(skipped — no live socket)\n"),
            InitializeCheck::Ok(v) => {
                s.push_str(&format!("OK (helix protocol_version={})\n", v))
            }
            InitializeCheck::Failed(e) => s.push_str(&format!("FAILED: {}\n", e)),
        }
        s
    }
}

/// Run the doctor. Always returns Ok — a diagnostic that crashes is
/// worse than useless.
pub async fn run() -> Result<(), anyhow::Error> {
    let report = collect_report().await;
    print!("{}", report.render());
    Ok(())
}

async fn collect_report() -> Report {
    let binary_on_path = which::which("helix-mcp").ok();
    let workspace = resources::resolve_workspace(None).ok();

    let snapshot = match &workspace {
        Some(ws) => check_snapshot(ws),
        None => SnapshotCheck::Missing(PathBuf::from("(no workspace)")),
    };

    let (socket, initialize) =
        match discovery::find_helix_socket(workspace.as_deref()).await {
            Ok(sock) => {
                let init = probe_initialize(&sock).await;
                (SocketCheck::Live(sock), init)
            }
            Err(_) => (SocketCheck::None, InitializeCheck::Skipped),
        };

    Report {
        binary_on_path,
        workspace,
        snapshot,
        socket,
        initialize,
    }
}

fn check_snapshot(workspace: &Path) -> SnapshotCheck {
    let path = workspace.join(".helix").join("context.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return SnapshotCheck::Missing(path);
        }
        Err(e) => return SnapshotCheck::Unreadable(path, e.to_string()),
    };
    let snap: ContextSnapshot = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => return SnapshotCheck::InvalidJson(path, e.to_string()),
    };
    let age_secs = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    SnapshotCheck::Found {
        path,
        schema_version: snap.schema_version,
        age_secs,
    }
}

async fn probe_initialize(socket: &Path) -> InitializeCheck {
    let req = ControlRequest::Initialize {
        protocol_version: helix_context_schema::PROTOCOL_VERSION.into(),
        client_info: ClientInfo {
            name: "helix-mcp doctor".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    };
    // Short timeout — `doctor` is interactive; a 30s hang is unfriendly.
    match rpc_client::send_request_with_timeout(socket, &req, Duration::from_secs(5))
        .await
    {
        Ok(ControlResponse::Initialize {
            protocol_version, ..
        }) => InitializeCheck::Ok(protocol_version),
        Ok(_) => InitializeCheck::Failed("unexpected response variant".into()),
        Err(e) => InitializeCheck::Failed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_renders_all_checks_when_everything_ok() {
        let r = Report {
            binary_on_path: Some("/Users/test/.cargo/bin/helix-mcp".into()),
            workspace: Some(PathBuf::from("/tmp/ws")),
            snapshot: SnapshotCheck::Found {
                path: "/tmp/ws/.helix/context.json".into(),
                schema_version: 2,
                age_secs: 12,
            },
            socket: SocketCheck::Live("/tmp/ws/.helix/control-123.sock".into()),
            initialize: InitializeCheck::Ok("1.0".into()),
        };
        let s = r.render();
        for needle in [
            "binary on PATH",
            "workspace",
            "snapshot",
            "control socket",
            "initialize handshake",
            "/Users/test/.cargo/bin/helix-mcp",
            "/tmp/ws",
            "schema_version=2",
            "12s old",
            "control-123.sock",
            "protocol_version=1.0",
        ] {
            assert!(s.contains(needle), "rendered report missing '{}':\n{}", needle, s);
        }
    }

    #[test]
    fn report_renders_failure_states() {
        let r = Report {
            binary_on_path: None,
            workspace: None,
            snapshot: SnapshotCheck::Missing("/no/such/path".into()),
            socket: SocketCheck::None,
            initialize: InitializeCheck::Skipped,
        };
        let s = r.render();
        assert!(s.contains("NOT FOUND"), "should describe missing binary");
        assert!(s.contains("unable to resolve"), "should describe missing workspace");
        assert!(s.contains("MISSING"), "should describe missing snapshot");
        assert!(s.contains("(skipped"), "should describe skipped initialize");
    }

    #[test]
    fn report_renders_invalid_json_and_unreadable() {
        let r = Report {
            binary_on_path: None,
            workspace: Some(PathBuf::from("/x")),
            snapshot: SnapshotCheck::InvalidJson("/x/.helix/context.json".into(), "trailing comma at line 5".into()),
            socket: SocketCheck::None,
            initialize: InitializeCheck::Skipped,
        };
        let s = r.render();
        assert!(s.contains("INVALID JSON"));
        assert!(s.contains("trailing comma at line 5"));
    }

    #[test]
    fn check_snapshot_reports_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        match check_snapshot(tmp.path()) {
            SnapshotCheck::Missing(p) => assert!(p.ends_with("context.json")),
            other => panic!("expected Missing, got something else: {}", match other {
                SnapshotCheck::Missing(_) => "Missing",
                SnapshotCheck::Unreadable(_, _) => "Unreadable",
                SnapshotCheck::InvalidJson(_, _) => "InvalidJson",
                SnapshotCheck::Found { .. } => "Found",
            }),
        }
    }

    #[test]
    fn check_snapshot_reports_invalid_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".helix")).unwrap();
        std::fs::write(tmp.path().join(".helix").join("context.json"), "{ not json").unwrap();
        assert!(matches!(check_snapshot(tmp.path()), SnapshotCheck::InvalidJson(_, _)));
    }

    #[test]
    fn check_snapshot_reports_found_with_schema_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".helix")).unwrap();
        let body = serde_json::json!({
            "schema_version": 2,
            "min_supported_reader": 1,
            "timestamp": "2026-05-14T10:00:00Z",
            "last_update_source": "focus_lost",
            "project_root": "/tmp/test",
            "mode": "normal",
            "active": {
                "path": "main.rs",
                "modified": false,
                "line_count": 5,
                "cursors": [{"primary": true, "line": 1, "column": 1}],
                "selections": []
            },
            "open_buffers": []
        });
        std::fs::write(
            tmp.path().join(".helix").join("context.json"),
            body.to_string(),
        )
        .unwrap();
        match check_snapshot(tmp.path()) {
            SnapshotCheck::Found { schema_version, .. } => {
                assert_eq!(schema_version, 2);
            }
            _ => panic!("expected Found"),
        }
    }
}
