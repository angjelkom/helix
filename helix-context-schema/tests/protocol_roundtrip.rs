use helix_context_schema::{JsonRpcError, JsonRpcErrorCode};

#[test]
fn json_rpc_error_serializes_with_code_and_message() {
    let err = JsonRpcError {
        code: JsonRpcErrorCode::MethodNotFound,
        message: "Method 'foo' not found".into(),
        data: None,
    };
    let s = serde_json::to_string(&err).unwrap();
    assert!(s.contains(r#""code":-32601"#), "got: {}", s);
    assert!(s.contains(r#""message":"Method 'foo' not found""#), "got: {}", s);
    assert!(!s.contains(r#""data""#), "data should be omitted when None: {}", s);
}

#[test]
fn json_rpc_error_codes_match_jsonrpc_spec() {
    assert_eq!(JsonRpcErrorCode::ParseError as i32, -32700);
    assert_eq!(JsonRpcErrorCode::InvalidRequest as i32, -32600);
    assert_eq!(JsonRpcErrorCode::MethodNotFound as i32, -32601);
    assert_eq!(JsonRpcErrorCode::InvalidParams as i32, -32602);
    assert_eq!(JsonRpcErrorCode::InternalError as i32, -32603);
}

#[test]
fn json_rpc_custom_codes_in_helix_range() {
    assert_eq!(JsonRpcErrorCode::NoLspForLanguage as i32, -32001);
    assert_eq!(JsonRpcErrorCode::LspTimeout as i32, -32002);
    assert_eq!(JsonRpcErrorCode::BufferModeUnsafe as i32, -32003);
    assert_eq!(JsonRpcErrorCode::NoActiveDocument as i32, -32004);
    assert_eq!(JsonRpcErrorCode::PathOutsideWorkspace as i32, -32005);
}

use helix_context_schema::{
    ClientInfo, ControlRequest, ControlResponse, ServerCapabilities, ServerInfo,
    PROTOCOL_VERSION,
};

use helix_context_schema::{Active, Cursor, LineRange, OpenBuffer};

#[test]
fn current_state_request_serializes_with_empty_params() {
    let req = ControlRequest::CurrentState {};
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "current-state");
    assert_eq!(j["params"], serde_json::json!({}));
}

#[test]
fn current_state_response_round_trips() {
    let resp = ControlResponse::CurrentState {
        active: Active {
            path: Some("src/main.rs".into()),
            path_abs: Some("/repo/src/main.rs".into()),
            language: Some("rust".into()),
            modified: false,
            line_count: 100,
            cursors: vec![Cursor { primary: true, line: 1, column: 1 }],
            selections: vec![],
            text: None,
        },
        mode: "normal".into(),
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "current-state");
    assert_eq!(j["result"]["mode"], "normal");
    assert_eq!(j["result"]["active"]["language"], "rust");

    let back: ControlResponse = serde_json::from_value(j).unwrap();
    let ControlResponse::CurrentState { mode, active } = back else {
        panic!("wrong variant");
    };
    assert_eq!(mode, "normal");
    assert_eq!(active.language.as_deref(), Some("rust"));
}

#[test]
fn get_open_buffers_request_and_response_round_trip() {
    let req = ControlRequest::GetOpenBuffers {};
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-open-buffers");

    let resp = ControlResponse::GetOpenBuffers {
        buffers: vec![OpenBuffer {
            path: Some("src/lib.rs".into()),
            language: Some("rust".into()),
            modified: false,
        }],
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "get-open-buffers");
    assert_eq!(j["result"]["buffers"][0]["path"], "src/lib.rs");
}

#[test]
fn get_buffer_text_request_with_path_and_range() {
    let req = ControlRequest::GetBufferText {
        path: Some("src/main.rs".into()),
        range: Some(LineRange { start_line: 10, end_line: 20 }),
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-buffer-text");
    assert_eq!(j["params"]["path"], "src/main.rs");
    assert_eq!(j["params"]["range"]["start_line"], 10);
    assert_eq!(j["params"]["range"]["end_line"], 20);

    let back: ControlRequest = serde_json::from_value(j).unwrap();
    let ControlRequest::GetBufferText { path, range } = back else {
        panic!("wrong variant");
    };
    assert_eq!(path.as_deref(), Some("src/main.rs"));
    let r = range.expect("range expected");
    assert_eq!(r.start_line, 10);
    assert_eq!(r.end_line, 20);
}

#[test]
fn get_buffer_text_request_with_no_path_omits_field() {
    let req = ControlRequest::GetBufferText { path: None, range: None };
    let j = serde_json::to_value(&req).unwrap();
    assert!(j["params"].get("path").is_none() || j["params"]["path"].is_null());
    assert!(j["params"].get("range").is_none() || j["params"]["range"].is_null());
}

#[test]
fn get_buffer_text_response_round_trips() {
    let resp = ControlResponse::GetBufferText {
        text: "fn main() {}".into(),
        language: Some("rust".into()),
        line_count: 1,
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["result"]["text"], "fn main() {}");
    assert_eq!(j["result"]["line_count"], 1);
}

#[test]
fn initialize_request_serializes_with_method_tag() {
    let req = ControlRequest::Initialize {
        protocol_version: PROTOCOL_VERSION.into(),
        client_info: ClientInfo {
            name: "helix-claude-mcp".into(),
            version: "0.1.0".into(),
        },
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "initialize");
    assert_eq!(j["params"]["protocol_version"], "1.0");
    assert_eq!(j["params"]["client_info"]["name"], "helix-claude-mcp");
}

#[test]
fn control_request_deserializes_initialize_from_method_tag() {
    let json = serde_json::json!({
        "method": "initialize",
        "params": {
            "protocol_version": "1.0",
            "client_info": { "name": "test", "version": "0.1.0" }
        }
    });
    let req: ControlRequest = serde_json::from_value(json).unwrap();
    let ControlRequest::Initialize { protocol_version, client_info } = req else {
        panic!("expected Initialize variant");
    };
    assert_eq!(protocol_version, "1.0");
    assert_eq!(client_info.name, "test");
}

#[test]
fn initialize_response_round_trips() {
    let resp = ControlResponse::Initialize {
        protocol_version: PROTOCOL_VERSION.into(),
        helix_version: "25.7.1".into(),
        server_info: ServerInfo {
            name: "helix".into(),
            version: "25.7.1".into(),
        },
        capabilities: ServerCapabilities {
            read_methods: vec!["initialize".into()],
            write_methods: vec![],
        },
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["result"]["protocol_version"], "1.0");
    assert_eq!(j["result"]["helix_version"], "25.7.1");
    let back: ControlResponse = serde_json::from_value(j).unwrap();
    let ControlResponse::Initialize { helix_version, .. } = back else {
        panic!("expected Initialize variant");
    };
    assert_eq!(helix_version, "25.7.1");
}

#[test]
fn open_file_request_serializes_with_path() {
    let req = ControlRequest::OpenFile { path: "src/main.rs".into() };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "open-file");
    assert_eq!(j["params"]["path"], "src/main.rs");
}

#[test]
fn open_file_request_round_trips() {
    let json = serde_json::json!({
        "method": "open-file",
        "params": { "path": "src/lib.rs" }
    });
    let req: ControlRequest = serde_json::from_value(json).unwrap();
    let ControlRequest::OpenFile { path } = req else {
        panic!("wrong variant");
    };
    assert_eq!(path, "src/lib.rs");
}

#[test]
fn goto_line_request_with_only_line() {
    let req = ControlRequest::GotoLine { line: 42, column: None, path: None };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "goto-line");
    assert_eq!(j["params"]["line"], 42);
    assert!(j["params"].get("column").is_none() || j["params"]["column"].is_null());
    assert!(j["params"].get("path").is_none() || j["params"]["path"].is_null());
}

#[test]
fn goto_line_request_with_all_fields() {
    let req = ControlRequest::GotoLine {
        line: 10,
        column: Some(5),
        path: Some("src/main.rs".into()),
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["params"]["line"], 10);
    assert_eq!(j["params"]["column"], 5);
    assert_eq!(j["params"]["path"], "src/main.rs");
}

#[test]
fn ok_response_serializes_with_empty_result() {
    let resp = ControlResponse::Ok {};
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "ok");
    assert_eq!(j["result"], serde_json::json!({}));
}
