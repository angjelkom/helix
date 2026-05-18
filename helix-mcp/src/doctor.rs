//! `doctor` subcommand. Runs the five checks a freshly-installed bridge
//! has to pass to be useful: binary on PATH, workspace resolvable,
//! snapshot file present and parseable, control socket connectable,
//! `initialize` handshake succeeds. Prints a human-readable report and
//! returns Ok regardless of outcome — diagnostics never fail.

use std::path::{Path, PathBuf};
use std::time::Duration;

use helix_context_schema::{ClientInfo, ContextSnapshot, ControlRequest, ControlResponse};

use crate::{discovery, resources, rpc_client};

/// Aggregated outcome of the doctor's checks. Pure data; rendering is on
/// `Self::render` for ease of unit testing.
pub struct Report {
    pub binary_on_path: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub snapshot: SnapshotCheck,
    pub socket: SocketCheck,
    pub initialize: InitializeCheck,
    pub config: ConfigCheck,
}

/// Onboarding-time config sanity. None of these block the bridge from
/// working — they surface "you forgot to enable feature X" so new users
/// don't have to grep three config files in three formats.
pub struct ConfigCheck {
    pub helix_config: HelixConfigCheck,
    pub mcp_json: McpJsonCheck,
    pub claude_settings: ClaudeSettingsCheck,
}

pub enum HelixConfigCheck {
    Missing(PathBuf),
    Unreadable(PathBuf, String),
    InvalidToml(PathBuf, String),
    Parsed {
        path: PathBuf,
        /// `[editor.context-logger] enabled = ?`. `None` when the table
        /// isn't present (Helix's default is enabled).
        context_logger_enabled: Option<bool>,
        /// `[editor.control-socket] enabled = ?`. `None` when the table
        /// isn't present (Helix's default is enabled).
        control_socket_enabled: Option<bool>,
    },
}

pub enum McpJsonCheck {
    Missing(PathBuf),
    Unreadable(PathBuf, String),
    InvalidJson(PathBuf, String),
    Parsed {
        path: PathBuf,
        /// True when any `mcpServers` entry's command is `helix-mcp` or
        /// ends with `helix-mcp` (path or just-the-binary forms).
        references_helix_mcp: bool,
    },
}

pub enum ClaudeSettingsCheck {
    Missing(PathBuf),
    Unreadable(PathBuf, String),
    InvalidJson(PathBuf, String),
    Parsed {
        path: PathBuf,
        /// True when `hooks.UserPromptSubmit` is a non-empty array.
        has_userpromptsubmit_hook: bool,
    },
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

        s.push_str("\nConfig files\n------------\n");
        s.push_str("helix config        : ");
        match &self.config.helix_config {
            HelixConfigCheck::Missing(p) => {
                s.push_str(&format!("MISSING at {}\n", p.display()))
            }
            HelixConfigCheck::Unreadable(p, e) => {
                s.push_str(&format!("UNREADABLE at {}: {}\n", p.display(), e))
            }
            HelixConfigCheck::InvalidToml(p, e) => {
                s.push_str(&format!("INVALID TOML at {}: {}\n", p.display(), e))
            }
            HelixConfigCheck::Parsed {
                path,
                context_logger_enabled,
                control_socket_enabled,
            } => {
                s.push_str(&format!("OK at {}\n", path.display()));
                s.push_str(&format!(
                    "  [editor.context-logger]    : {}\n",
                    enabled_label(*context_logger_enabled),
                ));
                s.push_str(&format!(
                    "  [editor.control-socket]    : {}\n",
                    enabled_label(*control_socket_enabled),
                ));
            }
        }
        s.push_str(".mcp.json           : ");
        match &self.config.mcp_json {
            McpJsonCheck::Missing(p) => s.push_str(&format!("MISSING at {}\n", p.display())),
            McpJsonCheck::Unreadable(p, e) => {
                s.push_str(&format!("UNREADABLE at {}: {}\n", p.display(), e))
            }
            McpJsonCheck::InvalidJson(p, e) => {
                s.push_str(&format!("INVALID JSON at {}: {}\n", p.display(), e))
            }
            McpJsonCheck::Parsed { path, references_helix_mcp } => {
                s.push_str(&format!(
                    "OK at {} ({})\n",
                    path.display(),
                    if *references_helix_mcp {
                        "references helix-mcp"
                    } else {
                        "no helix-mcp entry"
                    }
                ));
            }
        }
        s.push_str("claude settings     : ");
        match &self.config.claude_settings {
            ClaudeSettingsCheck::Missing(p) => {
                s.push_str(&format!("MISSING at {}\n", p.display()))
            }
            ClaudeSettingsCheck::Unreadable(p, e) => {
                s.push_str(&format!("UNREADABLE at {}: {}\n", p.display(), e))
            }
            ClaudeSettingsCheck::InvalidJson(p, e) => {
                s.push_str(&format!("INVALID JSON at {}: {}\n", p.display(), e))
            }
            ClaudeSettingsCheck::Parsed {
                path,
                has_userpromptsubmit_hook,
            } => {
                s.push_str(&format!(
                    "OK at {} ({})\n",
                    path.display(),
                    if *has_userpromptsubmit_hook {
                        "UserPromptSubmit hook wired"
                    } else {
                        "no UserPromptSubmit hook"
                    }
                ));
            }
        }

        let warnings = self.cross_check_warnings();
        if !warnings.is_empty() {
            s.push_str("\nWarnings\n--------\n");
            for w in &warnings {
                s.push_str("- ");
                s.push_str(w);
                s.push('\n');
            }
        }

        s
    }

    /// Cross-check the parsed config files for inconsistencies that
    /// reliably break the bridge for new users. Returns a list of
    /// human-readable warning lines (empty if all is well).
    pub fn cross_check_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        let control_socket_disabled = matches!(
            &self.config.helix_config,
            HelixConfigCheck::Parsed { control_socket_enabled: Some(false), .. },
        );
        let context_logger_disabled = matches!(
            &self.config.helix_config,
            HelixConfigCheck::Parsed { context_logger_enabled: Some(false), .. },
        );
        let mcp_references_helix = matches!(
            &self.config.mcp_json,
            McpJsonCheck::Parsed { references_helix_mcp: true, .. },
        );

        if mcp_references_helix && control_socket_disabled {
            warnings.push(
                ".mcp.json registers helix-mcp but `[editor.control-socket] enabled = false` \
                 — every write tool will fail with `Helix is not running in this workspace`. \
                 Set enabled = true (or remove the override) in your Helix config."
                    .into(),
            );
        }

        if context_logger_disabled {
            warnings.push(
                "`[editor.context-logger] enabled = false` disables the snapshot file. \
                 The MCP `helix://state/*` resources will return stale data; remove the \
                 override to re-enable."
                    .into(),
            );
        }

        warnings
    }
}

fn enabled_label(opt: Option<bool>) -> &'static str {
    match opt {
        Some(true) => "enabled = true",
        Some(false) => "enabled = FALSE (override)",
        None => "default (enabled)",
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

    let config = ConfigCheck {
        helix_config: check_helix_config(),
        mcp_json: match &workspace {
            Some(ws) => check_mcp_json(ws),
            None => McpJsonCheck::Missing(PathBuf::from("(no workspace)")),
        },
        claude_settings: check_claude_settings(),
    };

    Report {
        binary_on_path,
        workspace,
        snapshot,
        socket,
        initialize,
        config,
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

/// Resolve `~/.config/helix/config.toml` honoring `$XDG_CONFIG_HOME`.
/// Returns `None` if neither `$XDG_CONFIG_HOME` nor `$HOME` is set
/// (rare; happens in stripped-down sandboxes).
fn helix_config_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("helix").join("config.toml"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join("helix").join("config.toml"))
}

fn claude_settings_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".claude").join("settings.json"))
}

fn check_helix_config() -> HelixConfigCheck {
    let path = match helix_config_path() {
        Some(p) => p,
        None => return HelixConfigCheck::Missing(PathBuf::from("(no $HOME)")),
    };
    check_helix_config_at(path)
}

/// Inner half of `check_helix_config` that takes the path directly,
/// for tests that need to point at a tmpdir without racing on
/// `XDG_CONFIG_HOME` via the env.
fn check_helix_config_at(path: PathBuf) -> HelixConfigCheck {
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return HelixConfigCheck::Missing(path);
        }
        Err(e) => return HelixConfigCheck::Unreadable(path, e.to_string()),
    };
    let value: toml::Value = match toml::from_str(&text) {
        Ok(v) => v,
        Err(e) => return HelixConfigCheck::InvalidToml(path, e.to_string()),
    };
    let editor = value.get("editor").and_then(|v| v.as_table());
    let context_logger_enabled = editor
        .and_then(|t| t.get("context-logger"))
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("enabled"))
        .and_then(|v| v.as_bool());
    let control_socket_enabled = editor
        .and_then(|t| t.get("control-socket"))
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("enabled"))
        .and_then(|v| v.as_bool());
    HelixConfigCheck::Parsed {
        path,
        context_logger_enabled,
        control_socket_enabled,
    }
}

fn check_mcp_json(workspace: &Path) -> McpJsonCheck {
    let path = workspace.join(".mcp.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return McpJsonCheck::Missing(path);
        }
        Err(e) => return McpJsonCheck::Unreadable(path, e.to_string()),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return McpJsonCheck::InvalidJson(path, e.to_string()),
    };
    let references_helix_mcp = value
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .map(|servers| {
            servers.values().any(|entry| {
                entry
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(|cmd| cmd == "helix-mcp" || cmd.ends_with("/helix-mcp"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    McpJsonCheck::Parsed {
        path,
        references_helix_mcp,
    }
}

fn check_claude_settings() -> ClaudeSettingsCheck {
    let path = match claude_settings_path() {
        Some(p) => p,
        None => return ClaudeSettingsCheck::Missing(PathBuf::from("(no $HOME)")),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return ClaudeSettingsCheck::Missing(path);
        }
        Err(e) => return ClaudeSettingsCheck::Unreadable(path, e.to_string()),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return ClaudeSettingsCheck::InvalidJson(path, e.to_string()),
    };
    let has_userpromptsubmit_hook = value
        .get("hooks")
        .and_then(|v| v.get("UserPromptSubmit"))
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    ClaudeSettingsCheck::Parsed {
        path,
        has_userpromptsubmit_hook,
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

    /// Default ConfigCheck used in core-report tests where only the
    /// five original checks are exercised. Keeps those tests focused
    /// without recapitulating the config payload each time.
    fn empty_config() -> ConfigCheck {
        ConfigCheck {
            helix_config: HelixConfigCheck::Missing("(test)".into()),
            mcp_json: McpJsonCheck::Missing("(test)".into()),
            claude_settings: ClaudeSettingsCheck::Missing("(test)".into()),
        }
    }

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
            config: empty_config(),
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
            config: empty_config(),
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
            config: empty_config(),
        };
        let s = r.render();
        assert!(s.contains("INVALID JSON"));
        assert!(s.contains("trailing comma at line 5"));
    }

    #[test]
    fn check_helix_config_reports_missing_when_file_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let check = check_helix_config_at(tmp.path().join("config.toml"));
        assert!(matches!(check, HelixConfigCheck::Missing(_)));
    }

    #[test]
    fn check_helix_config_parses_enabled_overrides() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[editor.context-logger]
enabled = false

[editor.control-socket]
enabled = true
"#,
        )
        .unwrap();
        match check_helix_config_at(path) {
            HelixConfigCheck::Parsed {
                context_logger_enabled,
                control_socket_enabled,
                ..
            } => {
                assert_eq!(context_logger_enabled, Some(false));
                assert_eq!(control_socket_enabled, Some(true));
            }
            other => panic!(
                "expected Parsed, got {}",
                match other {
                    HelixConfigCheck::Missing(_) => "Missing",
                    HelixConfigCheck::Unreadable(_, _) => "Unreadable",
                    HelixConfigCheck::InvalidToml(_, _) => "InvalidToml",
                    HelixConfigCheck::Parsed { .. } => "Parsed (impossible)",
                }
            ),
        }
    }

    #[test]
    fn check_helix_config_reports_invalid_toml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "[editor.").unwrap();
        assert!(matches!(check_helix_config_at(path), HelixConfigCheck::InvalidToml(_, _)));
    }

    #[test]
    fn check_mcp_json_detects_helix_mcp_reference() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(".mcp.json"),
            r#"{"mcpServers": {"helix": {"command": "helix-mcp", "args": ["serve"]}}}"#,
        )
        .unwrap();
        match check_mcp_json(tmp.path()) {
            McpJsonCheck::Parsed { references_helix_mcp, .. } => {
                assert!(references_helix_mcp);
            }
            _ => panic!("expected Parsed"),
        }
    }

    #[test]
    fn check_mcp_json_handles_absolute_path_command() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(".mcp.json"),
            r#"{"mcpServers": {"h": {"command": "/Users/test/.cargo/bin/helix-mcp"}}}"#,
        )
        .unwrap();
        match check_mcp_json(tmp.path()) {
            McpJsonCheck::Parsed { references_helix_mcp, .. } => {
                assert!(references_helix_mcp);
            }
            _ => panic!("expected Parsed"),
        }
    }

    #[test]
    fn check_mcp_json_reports_missing_when_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(matches!(check_mcp_json(tmp.path()), McpJsonCheck::Missing(_)));
    }

    #[test]
    fn check_mcp_json_no_helix_entry_when_other_servers_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(".mcp.json"),
            r#"{"mcpServers": {"slack": {"command": "slack-mcp"}}}"#,
        )
        .unwrap();
        match check_mcp_json(tmp.path()) {
            McpJsonCheck::Parsed { references_helix_mcp, .. } => {
                assert!(!references_helix_mcp);
            }
            _ => panic!("expected Parsed"),
        }
    }

    #[test]
    fn cross_check_warns_when_mcp_references_bridge_but_socket_disabled() {
        let r = Report {
            binary_on_path: None,
            workspace: Some(PathBuf::from("/x")),
            snapshot: SnapshotCheck::Missing("/x/.helix/context.json".into()),
            socket: SocketCheck::None,
            initialize: InitializeCheck::Skipped,
            config: ConfigCheck {
                helix_config: HelixConfigCheck::Parsed {
                    path: "/cfg".into(),
                    context_logger_enabled: None,
                    control_socket_enabled: Some(false),
                },
                mcp_json: McpJsonCheck::Parsed {
                    path: "/x/.mcp.json".into(),
                    references_helix_mcp: true,
                },
                claude_settings: ClaudeSettingsCheck::Missing("(test)".into()),
            },
        };
        let warnings = r.cross_check_warnings();
        assert!(
            warnings.iter().any(|w| w.contains("control-socket")),
            "expected a control-socket warning, got: {:?}",
            warnings
        );
    }

    #[test]
    fn cross_check_warns_when_context_logger_disabled() {
        let r = Report {
            binary_on_path: None,
            workspace: None,
            snapshot: SnapshotCheck::Missing("(no workspace)".into()),
            socket: SocketCheck::None,
            initialize: InitializeCheck::Skipped,
            config: ConfigCheck {
                helix_config: HelixConfigCheck::Parsed {
                    path: "/cfg".into(),
                    context_logger_enabled: Some(false),
                    control_socket_enabled: None,
                },
                mcp_json: McpJsonCheck::Missing("(test)".into()),
                claude_settings: ClaudeSettingsCheck::Missing("(test)".into()),
            },
        };
        let warnings = r.cross_check_warnings();
        assert!(
            warnings.iter().any(|w| w.contains("context-logger")),
            "expected a context-logger warning, got: {:?}",
            warnings
        );
    }

    #[test]
    fn cross_check_silent_when_all_defaults() {
        let r = Report {
            binary_on_path: None,
            workspace: None,
            snapshot: SnapshotCheck::Missing("(no workspace)".into()),
            socket: SocketCheck::None,
            initialize: InitializeCheck::Skipped,
            config: ConfigCheck {
                helix_config: HelixConfigCheck::Parsed {
                    path: "/cfg".into(),
                    context_logger_enabled: None,
                    control_socket_enabled: None,
                },
                mcp_json: McpJsonCheck::Parsed {
                    path: "/x/.mcp.json".into(),
                    references_helix_mcp: true,
                },
                claude_settings: ClaudeSettingsCheck::Parsed {
                    path: "/h".into(),
                    has_userpromptsubmit_hook: true,
                },
            },
        };
        assert!(r.cross_check_warnings().is_empty());
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
