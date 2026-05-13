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
