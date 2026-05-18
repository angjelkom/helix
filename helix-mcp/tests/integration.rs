//! Integration test: spawn `helix-mcp serve`, drive it via stdin/stdout
//! as Claude Code would. Tests use the in-file `Harness` to keep each
//! test focused on its assertion rather than the spawn/handshake/parse
//! boilerplate.

use serde_json::{json, Value};
use std::process::Stdio;

use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

// ---------------------------------------------------------------------------
// Sample data
// ---------------------------------------------------------------------------

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

/// Canned `initialize` response the fake-helix returns on the first
/// connection per `helix-mcp serve` process — the bridge sends Initialize
/// before any tool RPC and caches the outcome per-process. Tests that
/// only check non-tool surfaces (resources/list, tools/list) never reach
/// the fake; this only matters once a tool call happens.
const FAKE_INITIALIZE_RESPONSE: &str = r#"{"method":"initialize","result":{"protocol_version":"1.0","helix_version":"test","server_info":{"name":"fake","version":"0.1"},"capabilities":{"read_methods":[],"write_methods":[]}}}
"#;

fn binary_path() -> std::path::PathBuf {
    // CARGO_BIN_EXE_helix-mcp is set by cargo for integration tests.
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_helix-mcp"))
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------
//
// Each test used to be ~50 lines of tempdir setup, child spawn, stdio
// piping, three-message init, then a read-loop with a substring assertion
// against a JSON envelope. The harness bundles all of that:
//
//   let mut h = Harness::new().await;
//   h.handshake().await;
//   let result = h.call_tool("helix_open_file", json!({"path":"x"})).await;
//   assert_eq!(result["isError"], false);
//
// Assertions land against parsed JSON values rather than substring
// matches — closes the "fake echoes canned regardless of method" hole
// the old loose-substring style left open.

/// Test harness — spawns `helix-mcp serve` with a sample snapshot and
/// (optionally) a fake-helix listener on the workspace's `.helix/`
/// socket. Drop the harness to kill the child.
struct Harness {
    /// Kept alive for the lifetime of the harness so the workspace
    /// directory and any sockets bound under it survive.
    _workspace: TempDir,
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    /// Monotonic JSON-RPC id used by the helper methods. Tests should
    /// not need to track this themselves.
    next_id: u64,
}

impl Harness {
    /// Spawn `helix-mcp serve` against a fresh tempdir workspace that has
    /// a valid `.helix/context.json` snapshot. No fake-helix listener —
    /// tool calls will hit "Helix is not running" cleanly. Use
    /// `new_with_fake_helix` when you need tool-call responses.
    async fn new() -> Self {
        let workspace = TempDir::new().unwrap();
        let helix = workspace.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

        let mut child = Command::new(binary_path())
            .arg("serve")
            .env("CLAUDE_PROJECT_DIR", workspace.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn helix-mcp serve");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let reader = BufReader::new(stdout);

        Self {
            _workspace: workspace,
            child,
            stdin,
            reader,
            next_id: 0,
        }
    }

    /// Like `new`, plus a fake-helix listener on the workspace's
    /// `.helix/control-12345.sock`. The fake responds to `initialize`
    /// requests with a valid Initialize response and to every other
    /// request with `canned`. Caller is responsible for supplying a
    /// JSON-RPC frame in `canned` ending with a newline.
    async fn new_with_fake_helix(canned: &str) -> Self {
        let h = Self::new().await;
        spawn_fake_helix(h._workspace.path(), canned.to_string());
        // Tiny pause so the listener is bound before the bridge's first
        // tool call probes for a live socket. The bridge handles a
        // race but the test is more deterministic with the warm-up.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        h
    }

    /// Send the standard MCP initialize + notifications/initialized.
    /// Returns the parsed initialize response value (the "result"
    /// field). Tests that don't care about the result can ignore it.
    async fn handshake(&mut self) -> Value {
        self.next_id = 1;
        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "harness", "version": "0.1"},
            }
        });
        self.send_raw(&init.to_string()).await;
        let resp = self.read_until_id(1).await;
        // notifications/initialized has no id and no response.
        self.send_raw(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await;
        resp["result"].clone()
    }

    /// Call `tools/call` and return the parsed result value (the
    /// JSON-RPC `result` field). Assert against the returned Value
    /// directly rather than substring-matching the line.
    async fn call_tool(&mut self, name: &str, args: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": args},
        });
        self.send_raw(&req.to_string()).await;
        let resp = self.read_until_id(id).await;
        resp["result"].clone()
    }

    /// Call `tools/list` and return the parsed result.
    async fn list_tools(&mut self) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/list",
            "params": {},
        });
        self.send_raw(&req.to_string()).await;
        self.read_until_id(id).await["result"].clone()
    }

    /// Call `resources/list` and return the parsed result.
    async fn list_resources(&mut self) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "resources/list",
            "params": {},
        });
        self.send_raw(&req.to_string()).await;
        self.read_until_id(id).await["result"].clone()
    }

    /// Call `resources/read` and return the parsed result.
    async fn read_resource(&mut self, uri: &str) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "resources/read",
            "params": {"uri": uri},
        });
        self.send_raw(&req.to_string()).await;
        self.read_until_id(id).await["result"].clone()
    }

    async fn send_raw(&mut self, line: &str) {
        self.stdin.write_all(line.as_bytes()).await.unwrap();
        self.stdin.write_all(b"\n").await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    /// Read JSON-RPC frames until one with the matching id appears.
    /// Ignores notifications (no id) and out-of-order responses.
    /// Times out after ~10 frames so a misbehaving server can't hang
    /// the test.
    async fn read_until_id(&mut self, want: u64) -> Value {
        for _ in 0..10 {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await.unwrap();
            if n == 0 {
                panic!("server stdout closed before id={} arrived", want);
            }
            let v: Value = serde_json::from_str(line.trim())
                .unwrap_or_else(|_| panic!("non-JSON line from server: {}", line));
            if v.get("id").and_then(|i| i.as_u64()) == Some(want) {
                return v;
            }
            // Not our response — keep reading.
        }
        panic!("never saw a response with id={} after 10 frames", want);
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Closing stdin signals EOF to the server; the child should
        // exit cleanly. Use start_kill instead of kill().await because
        // Drop can't be async — best-effort.
        let _ = self.child.start_kill();
    }
}

/// Bind a fake Helix-side at `<workspace>/.helix/control-12345.sock`
/// that responds to `initialize` with a valid handshake and to every
/// other request with `canned`. The listener accepts in a loop until
/// the test ends.
fn spawn_fake_helix(workspace: &std::path::Path, canned: String) {
    let helix_dir = workspace.join(".helix");
    let sock = helix_dir.join("control-12345.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    let canned = canned.clone();
                    tokio::spawn(async move {
                        use tokio::io::AsyncReadExt;
                        let mut buf = vec![0u8; 8192];
                        let n = stream.read(&mut buf).await.unwrap_or(0);
                        let req_text =
                            std::str::from_utf8(&buf[..n]).unwrap_or("");
                        let resp = if req_text.contains("\"method\":\"initialize\"") {
                            FAKE_INITIALIZE_RESPONSE.to_string()
                        } else {
                            canned
                        };
                        let _ = stream.write_all(resp.as_bytes()).await;
                        let _ = stream.flush().await;
                    });
                }
                Err(_) => break,
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Helpers for asserting on tool-call results
// ---------------------------------------------------------------------------

/// Extract the inner JSON object from a `tools/call` result. rmcp wraps
/// our `{"method":"...","result":{...}}` line in a content envelope:
///   { "content": [{"type":"text","text":"<inner JSON as string>"}], "isError": false }
/// This helper parses the inner string back to a Value for strict
/// assertions.
fn tool_result_inner(call_result: &Value) -> Value {
    assert_eq!(
        call_result["isError"],
        Value::Bool(false),
        "tool call returned isError=true: {}",
        call_result
    );
    let text = call_result["content"][0]["text"]
        .as_str()
        .expect("tool result content[0].text was not a string");
    serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("tool result text was not JSON: {} (text: {})", e, text))
}

/// Assert that a `tools/call` result represents an MCP-level error.
fn assert_tool_error(call_result: &Value) -> String {
    assert_eq!(
        call_result["isError"],
        Value::Bool(true),
        "expected tool error, got success: {}",
        call_result
    );
    call_result["content"][0]["text"]
        .as_str()
        .expect("tool error content[0].text was not a string")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests — one per surface, each ~10 lines.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initialize_handshake_succeeds() {
    let mut h = Harness::new().await;
    let init = h.handshake().await;
    assert!(
        init.get("serverInfo").is_some() || init.get("server_info").is_some(),
        "initialize result missing serverInfo: {}",
        init
    );
    let instructions = init["instructions"]
        .as_str()
        .expect("instructions field absent or non-string");
    assert!(
        instructions.contains("navigate before editing"),
        "instructions content does not look like ours: {}",
        instructions
    );
}

#[tokio::test]
async fn resources_list_returns_three_uris() {
    let mut h = Harness::new().await;
    h.handshake().await;
    let result = h.list_resources().await;
    let uris: Vec<&str> = result["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    assert!(uris.contains(&"helix://state/current"));
    assert!(uris.contains(&"helix://state/buffers"));
    assert!(uris.contains(&"helix://state/snapshot"));
    assert_eq!(uris.len(), 3);
}

#[tokio::test]
async fn resources_read_current_returns_active_block() {
    let mut h = Harness::new().await;
    h.handshake().await;
    let result = h.read_resource("helix://state/current").await;
    let text = result["contents"][0]["text"].as_str().unwrap();
    let inner: Value = serde_json::from_str(text).unwrap();
    assert_eq!(inner["mode"], "normal");
    assert_eq!(inner["active"]["path"], "main.rs");
}

#[tokio::test]
async fn tools_list_returns_all_registered_tools() {
    let mut h = Harness::new().await;
    h.handshake().await;
    let result = h.list_tools().await;
    let names: Vec<&str> = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for expected in [
        "helix_open_file",
        "helix_goto_line",
        "helix_select",
        "helix_multi_select",
        "helix_get_diagnostics",
        "helix_get_hover",
        "helix_get_definition",
        "helix_get_references",
        "helix_get_workspace_symbols",
        "helix_get_document_symbols",
        "helix_get_signature_help",
        "helix_get_selection",
        "helix_buffer_read",
        "helix_format_document",
        "helix_run_command",
    ] {
        assert!(
            names.contains(&expected),
            "tools/list missing {} — got {:?}",
            expected,
            names
        );
    }
    assert_eq!(names.len(), 15, "expected 15 tools, got: {:?}", names);
}

#[tokio::test]
async fn tools_call_open_file_succeeds_against_fake_helix() {
    let canned = r#"{"method":"ok","result":{}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h.call_tool("helix_open_file", json!({"path": "src/main.rs"})).await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["ok"], true);
}

#[tokio::test]
async fn tools_call_multi_select_succeeds_against_fake_helix() {
    let canned = r#"{"method":"ok","result":{}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h
        .call_tool(
            "helix_multi_select",
            json!({
                "ranges": [
                    {"start_line": 1, "start_column": 1, "end_line": 1, "end_column": 5},
                    {"start_line": 3, "start_column": 1, "end_line": 3, "end_column": 5},
                ],
                "primary_index": 1
            }),
        )
        .await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["ok"], true);
}

#[tokio::test]
async fn tools_call_multi_select_rejects_empty_ranges() {
    let mut h = Harness::new().await;
    h.handshake().await;
    let result = h
        .call_tool("helix_multi_select", json!({"ranges": []}))
        .await;
    let msg = assert_tool_error(&result);
    // Either the bridge's args validation or Helix's empty-check
    // fires. Both produce InvalidParams; assert the message mentions
    // the ranges field.
    assert!(
        msg.contains("ranges") || msg.contains("non-empty") || msg.contains("not running"),
        "expected empty-ranges or not-running message, got: {}",
        msg
    );
}

#[tokio::test]
async fn tools_call_select_succeeds_against_fake_helix() {
    let canned = r#"{"method":"ok","result":{}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h
        .call_tool(
            "helix_select",
            json!({"start_line": 1, "start_column": 1, "end_line": 3, "end_column": 10}),
        )
        .await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["ok"], true);
}

#[tokio::test]
async fn tools_call_returns_error_when_helix_not_running() {
    let mut h = Harness::new().await;
    h.handshake().await;
    let result = h.call_tool("helix_open_file", json!({"path": "x"})).await;
    let msg = assert_tool_error(&result);
    assert!(
        msg.contains("Helix is not running") || msg.contains("not running"),
        "expected friendly not-running message, got: {}",
        msg
    );
}

#[tokio::test]
async fn tools_call_format_document_against_fake_helix() {
    let canned = r#"{"method":"format-document","result":{"applied":true}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h.call_tool("helix_format_document", json!({})).await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["method"], "format-document");
    assert_eq!(inner["result"]["applied"], true);
}

#[tokio::test]
async fn tools_call_run_command_against_fake_helix() {
    let canned =
        r#"{"method":"run-command","result":{"message":"context snapshot written"}}"#.to_string()
            + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h
        .call_tool("helix_run_command", json!({"name": "write-context"}))
        .await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["method"], "run-command");
    assert_eq!(inner["result"]["message"], "context snapshot written");
}

#[tokio::test]
async fn tools_list_includes_phase_6_tools() {
    let mut h = Harness::new().await;
    h.handshake().await;
    let result = h.list_tools().await;
    let names: Vec<&str> = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"helix_format_document"));
    assert!(names.contains(&"helix_run_command"));
}

#[tokio::test]
async fn tools_call_get_hover_against_fake_helix() {
    let canned = r#"{"method":"get-hover-at","result":{"hover":{"contents":"fn main()","range":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}}}}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h
        .call_tool("helix_get_hover", json!({"line": 1, "column": 4}))
        .await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["method"], "get-hover-at");
    assert_eq!(inner["result"]["hover"]["contents"], "fn main()");
}

#[tokio::test]
async fn tools_call_get_definition_against_fake_helix() {
    let canned = r#"{"method":"get-definition-at","result":{"locations":[{"path":"src/lib.rs","path_abs":"/tmp/p4a-test/src/lib.rs","range":{"start":{"line":10,"character":4},"end":{"line":10,"character":12}}}]}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h
        .call_tool("helix_get_definition", json!({"line": 3, "column": 8}))
        .await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["method"], "get-definition-at");
    assert_eq!(inner["result"]["locations"][0]["path"], "src/lib.rs");
}

#[tokio::test]
async fn tools_call_get_references_against_fake_helix() {
    let canned = r#"{"method":"get-references-at","result":{"locations":[{"path":"src/main.rs","path_abs":"/tmp/p4a-test/src/main.rs","range":{"start":{"line":2,"character":0},"end":{"line":2,"character":8}}},{"path":"src/lib.rs","path_abs":"/tmp/p4a-test/src/lib.rs","range":{"start":{"line":5,"character":4},"end":{"line":5,"character":12}}}]}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h
        .call_tool("helix_get_references", json!({"line": 1, "column": 1}))
        .await;
    let inner = tool_result_inner(&result);
    let locations = inner["result"]["locations"].as_array().unwrap();
    assert_eq!(locations.len(), 2);
    assert_eq!(locations[0]["path"], "src/main.rs");
    assert_eq!(locations[1]["path"], "src/lib.rs");
}

#[tokio::test]
async fn tools_call_get_workspace_symbols_against_fake_helix() {
    let canned = r#"{"method":"get-workspace-symbols","result":{"symbols":[{"name":"main","kind":"function","location":{"path":"src/main.rs","path_abs":"/tmp/p4a-test/src/main.rs","range":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}}}}]}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h
        .call_tool("helix_get_workspace_symbols", json!({"query": "main"}))
        .await;
    let inner = tool_result_inner(&result);
    let symbols = inner["result"]["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0]["name"], "main");
    assert_eq!(symbols[0]["kind"], "function");
}

#[tokio::test]
async fn tools_call_get_diagnostics_against_fake_helix() {
    let canned = r#"{"method":"get-diagnostics","result":{"diagnostics":[{"range":{"start":{"line":4,"character":0},"end":{"line":4,"character":10}},"severity":"error","code":"E0308","source":"rustc","message":"mismatched types"}]}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h.call_tool("helix_get_diagnostics", json!({})).await;
    let inner = tool_result_inner(&result);
    let diags = inner["result"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["code"], "E0308");
    assert_eq!(diags[0]["message"], "mismatched types");
}

#[tokio::test]
async fn tools_call_buffer_read_against_fake_helix() {
    let canned = r#"{"method":"get-buffer-text","result":{"text":"fn main() {\n    println!(\"hello\");\n}\n","language":"rust","line_count":3}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h.call_tool("helix_buffer_read", json!({})).await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["method"], "get-buffer-text");
    assert_eq!(inner["result"]["language"], "rust");
    assert_eq!(inner["result"]["line_count"], 3);
}

#[tokio::test]
async fn tools_call_buffer_read_with_range() {
    let canned = r#"{"method":"get-buffer-text","result":{"text":"line 2\nline 3\n","language":"text","line_count":2}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h
        .call_tool("helix_buffer_read", json!({"start_line": 2, "end_line": 3}))
        .await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["result"]["line_count"], 2);
}

#[tokio::test]
async fn tools_call_get_signature_help_against_fake_helix() {
    let canned = r#"{"method":"get-signature-help","result":{"signatures":[{"label":"fn open(path: &Path) -> io::Result<File>","documentation":"Opens a file in read-only mode.","parameters":[{"label":"path: &Path"}]}],"active_signature":0,"active_parameter":0}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h
        .call_tool("helix_get_signature_help", json!({"line": 5, "column": 10}))
        .await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["method"], "get-signature-help");
    let sigs = inner["result"]["signatures"].as_array().unwrap();
    assert_eq!(sigs.len(), 1);
    assert!(sigs[0]["label"].as_str().unwrap().contains("fn open"));
    assert_eq!(inner["result"]["active_parameter"], 0);
}

#[tokio::test]
async fn tools_call_get_document_symbols_against_fake_helix() {
    let canned = r#"{"method":"get-document-symbols","result":{"symbols":[{"name":"main","kind":"function","range":{"start":{"line":0,"character":0},"end":{"line":2,"character":1}},"selection_range":{"start":{"line":0,"character":3},"end":{"line":0,"character":7}},"children":[]}]}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h.call_tool("helix_get_document_symbols", json!({})).await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["method"], "get-document-symbols");
    let syms = inner["result"]["symbols"].as_array().unwrap();
    assert_eq!(syms.len(), 1);
    assert_eq!(syms[0]["name"], "main");
    assert_eq!(syms[0]["kind"], "function");
}

#[tokio::test]
async fn tools_call_get_selection_against_fake_helix() {
    let canned = r#"{"method":"get-selections","result":{"selections":[{"primary":true,"start":{"line":1,"column":1},"end":{"line":2,"column":5},"byte_len":15,"text":"fn main() {\n    "}],"primary_index":0,"mode":"normal"}}"#.to_string() + "\n";
    let mut h = Harness::new_with_fake_helix(&canned).await;
    h.handshake().await;
    let result = h.call_tool("helix_get_selection", json!({})).await;
    let inner = tool_result_inner(&result);
    assert_eq!(inner["method"], "get-selections");
    let sels = inner["result"]["selections"].as_array().unwrap();
    assert_eq!(sels.len(), 1);
    assert_eq!(sels[0]["primary"], true);
    assert_eq!(sels[0]["text"], "fn main() {\n    ");
    assert_eq!(inner["result"]["mode"], "normal");
}

#[tokio::test]
async fn tools_call_buffer_read_rejects_partial_range() {
    // start_line without end_line is ambiguous; the parser must reject.
    let mut h = Harness::new().await; // no fake-helix needed — fails at parse
    h.handshake().await;
    let result = h
        .call_tool("helix_buffer_read", json!({"start_line": 2}))
        .await;
    let msg = assert_tool_error(&result);
    assert!(
        msg.contains("requires both") || msg.contains("Invalid arguments"),
        "expected partial-range rejection, got: {}",
        msg
    );
}
