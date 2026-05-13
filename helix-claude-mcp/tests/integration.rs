//! Integration test: spawn `helix-claude-mcp serve`, drive it via stdin/stdout
//! as Claude Code would.

use std::process::Stdio;

use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const SAMPLE_SNAPSHOT: &str = r#"{
  "schema_version": 2,
  "min_supported_reader": 1,
  "timestamp": "2026-05-13T10:00:00Z",
  "last_update_source": "focus_lost",
  "project_root": "/tmp/p4a-test",
  "mode": "normal",
  "active": {
    "path": "main.rs",
    "path_abs": "/tmp/p4a-test/main.rs",
    "language": "rust",
    "modified": false,
    "line_count": 5,
    "cursors": [{"primary": true, "line": 1, "column": 1}],
    "selections": []
  },
  "open_buffers": [
    {"path": "/tmp/p4a-test/main.rs", "language": "rust", "modified": false}
  ]
}"#;

fn binary_path() -> std::path::PathBuf {
    // CARGO_BIN_EXE_helix-claude-mcp is set by cargo for integration tests.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_helix-claude-mcp"))
}

#[tokio::test]
async fn initialize_handshake_succeeds() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    let mut child = Command::new(binary_path())
        .arg("serve")
        .env("CLAUDE_PROJECT_DIR", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helix-claude-mcp");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let init_req =
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#;
    stdin.write_all(init_req.as_bytes()).await.unwrap();
    stdin.write_all(b"\n").await.unwrap();
    stdin.flush().await.unwrap();

    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    assert!(line.contains("\"result\""), "initialize response: {}", line);
    assert!(
        line.contains("\"serverInfo\"") || line.contains("\"server_info\""),
        "missing serverInfo: {}",
        line,
    );

    // Tear down
    drop(stdin);
    let _ = child.kill().await;
}

#[tokio::test]
async fn resources_list_returns_three_uris() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    let mut child = Command::new(binary_path())
        .arg("serve")
        .env("CLAUDE_PROJECT_DIR", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // Send initialize, initialized notification, then resources/list
    for msg in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    // Read until we see the resources/list response (id: 2)
    let mut found = false;
    for _ in 0..5 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 {
                break;
            }
            if line.contains("\"id\":2") {
                assert!(
                    line.contains("helix://state/current"),
                    "missing current: {}",
                    line
                );
                assert!(
                    line.contains("helix://state/buffers"),
                    "missing buffers: {}",
                    line
                );
                assert!(
                    line.contains("helix://state/snapshot"),
                    "missing snapshot: {}",
                    line
                );
                found = true;
                break;
            }
        }
    }
    assert!(found, "didn't see resources/list response");

    drop(stdin);
    let _ = child.kill().await;
}

#[tokio::test]
async fn resources_read_current_returns_active_block() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    let mut child = Command::new(binary_path())
        .arg("serve")
        .env("CLAUDE_PROJECT_DIR", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    for msg in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":"helix://state/current"}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..5 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 {
                break;
            }
            if line.contains("\"id\":2") {
                // The result wraps the resource contents. Expect the JSON to
                // contain `main.rs` and `normal` (from active.path and mode).
                assert!(line.contains("main.rs"), "missing path: {}", line);
                assert!(line.contains("normal"), "missing mode: {}", line);
                found = true;
                break;
            }
        }
    }
    assert!(found, "didn't see resources/read response");

    drop(stdin);
    let _ = child.kill().await;
}
