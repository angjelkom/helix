//! Integration test: spawn `helix-claude-mcp serve`, drive it via stdin/stdout
//! as Claude Code would.

use std::process::Stdio;

use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::process::Command;

/// Bind a fake Helix-side at `<workspace>/.helix/control-12345.sock` that
/// accepts connections in a loop. For each connection: reads any input,
/// writes `canned_response_line`, then drops the connection. Keeps accepting
/// until the task is dropped. The first connect may be from discovery's
/// liveness probe (which just connects + drops); subsequent ones carry the
/// real RPC request. Returns the listener-bound socket path.
async fn spawn_fake_helix_in(
    workspace: &std::path::Path,
    canned_response_line: String,
) -> std::path::PathBuf {
    let helix_dir = workspace.join(".helix");
    std::fs::create_dir_all(&helix_dir).unwrap();
    let sock = helix_dir.join("control-12345.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let resp = canned_response_line.clone();
                    tokio::spawn(async move {
                        use tokio::io::AsyncReadExt;
                        let mut buf = vec![0u8; 8192];
                        let _ = stream.read(&mut buf).await;
                        let _ = stream.write_all(resp.as_bytes()).await;
                        let _ = stream.flush().await;
                    });
                }
                Err(_) => break,
            }
        }
    });
    sock
}

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

#[tokio::test]
async fn tools_list_returns_seven_tools() {
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
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..6 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                for tool in [
                    "helix_open_file", "helix_goto_line", "helix_get_diagnostics",
                    "helix_get_hover", "helix_get_definition", "helix_get_references",
                    "helix_get_workspace_symbols",
                ] {
                    assert!(line.contains(tool), "missing tool: {}\nfull line: {}", tool, line);
                }
                found = true;
                break;
            }
        }
    }
    assert!(found, "didn't see tools/list response");
    drop(stdin);
    let _ = child.kill().await;
}

#[tokio::test]
async fn tools_call_open_file_succeeds_against_fake_helix() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    // Fake Helix that responds to OpenFile with ControlResponse::Ok
    let canned = r#"{"method":"ok","result":{}}"#.to_string() + "\n";
    let _sock = spawn_fake_helix_in(tmp.path(), canned).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

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
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"helix_open_file","arguments":{"path":"src/main.rs"}}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..6 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                // Success response. Should contain the ok JSON in tool result content.
                assert!(line.contains("\"ok\"") || line.contains("ok"), "expected ok in response: {}", line);
                assert!(!line.contains("\"isError\":true"), "got error: {}", line);
                found = true;
                break;
            }
        }
    }
    assert!(found, "didn't see tools/call response");
    drop(stdin);
    let _ = child.kill().await;
}

#[tokio::test]
async fn tools_call_returns_error_when_helix_not_running() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();
    // NO socket bound — discovery will fail.

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
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"helix_open_file","arguments":{"path":"x"}}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..6 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                // The tool call should return is_error: true with a message
                // about Helix not running.
                assert!(
                    line.contains("Helix is not running") || line.contains("not running"),
                    "expected friendly Helix-not-running message: {}", line
                );
                found = true;
                break;
            }
        }
    }
    assert!(found, "didn't see tools/call error response");
    drop(stdin);
    let _ = child.kill().await;
}
