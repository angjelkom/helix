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

use helix_context_schema::{
    LspDiagnostic, LspHover, LspLocation, LspPosition, LspRange, LspSymbolInfo,
};

#[test]
fn lsp_position_serializes_zero_indexed() {
    let p = LspPosition { line: 0, character: 0 };
    let j = serde_json::to_value(&p).unwrap();
    assert_eq!(j["line"], 0);
    assert_eq!(j["character"], 0);
}

#[test]
fn lsp_range_round_trips() {
    let r = LspRange {
        start: LspPosition { line: 0, character: 5 },
        end: LspPosition { line: 2, character: 10 },
    };
    let j = serde_json::to_value(&r).unwrap();
    let back: LspRange = serde_json::from_value(j).unwrap();
    assert_eq!(back.start.line, 0);
    assert_eq!(back.end.character, 10);
}

#[test]
fn lsp_location_round_trips() {
    let loc = LspLocation {
        path: "src/main.rs".into(),
        path_abs: "/repo/src/main.rs".into(),
        range: LspRange {
            start: LspPosition { line: 0, character: 0 },
            end: LspPosition { line: 0, character: 5 },
        },
    };
    let j = serde_json::to_value(&loc).unwrap();
    assert_eq!(j["path"], "src/main.rs");
    let back: LspLocation = serde_json::from_value(j).unwrap();
    assert_eq!(back.path_abs, "/repo/src/main.rs");
}

#[test]
fn lsp_hover_omits_optional_range() {
    let h = LspHover { contents: "fn foo()".into(), range: None };
    let j = serde_json::to_value(&h).unwrap();
    assert!(j.get("range").is_none() || j["range"].is_null());
    assert_eq!(j["contents"], "fn foo()");
}

#[test]
fn lsp_diagnostic_serializes_with_all_fields() {
    let d = LspDiagnostic {
        range: LspRange {
            start: LspPosition { line: 5, character: 10 },
            end: LspPosition { line: 5, character: 15 },
        },
        severity: Some("error".into()),
        code: Some("E0308".into()),
        source: Some("rustc".into()),
        message: "expected `u32`, found `String`".into(),
    };
    let j = serde_json::to_value(&d).unwrap();
    assert_eq!(j["severity"], "error");
    assert_eq!(j["code"], "E0308");
    assert_eq!(j["source"], "rustc");
    assert_eq!(j["message"], "expected `u32`, found `String`");
    let back: LspDiagnostic = serde_json::from_value(j).unwrap();
    assert_eq!(back.range.start.line, 5);
}

#[test]
fn lsp_symbol_info_round_trips() {
    let s = LspSymbolInfo {
        name: "main".into(),
        kind: "function".into(),
        location: LspLocation {
            path: "src/main.rs".into(),
            path_abs: "/repo/src/main.rs".into(),
            range: LspRange {
                start: LspPosition { line: 0, character: 0 },
                end: LspPosition { line: 4, character: 1 },
            },
        },
        container_name: None,
    };
    let j = serde_json::to_value(&s).unwrap();
    assert_eq!(j["name"], "main");
    assert!(j.get("container_name").is_none() || j["container_name"].is_null());
    let back: LspSymbolInfo = serde_json::from_value(j).unwrap();
    assert_eq!(back.kind, "function");
}

#[test]
fn get_diagnostics_request_with_no_path() {
    let req = ControlRequest::GetDiagnostics { path: None };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-diagnostics");
    assert!(j["params"].get("path").is_none() || j["params"]["path"].is_null());
}

#[test]
fn get_hover_at_request_round_trips() {
    let req = ControlRequest::GetHoverAt {
        line: 10,
        column: 5,
        path: Some("src/main.rs".into()),
        allow_insert_mode: Some(false),
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-hover-at");
    assert_eq!(j["params"]["line"], 10);
    let back: ControlRequest = serde_json::from_value(j).unwrap();
    let ControlRequest::GetHoverAt { line, column, .. } = back else {
        panic!("wrong variant");
    };
    assert_eq!(line, 10);
    assert_eq!(column, 5);
}

#[test]
fn get_definition_at_request_omits_optional_fields() {
    let req = ControlRequest::GetDefinitionAt {
        line: 1,
        column: 1,
        path: None,
        allow_insert_mode: None,
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-definition-at");
    assert!(j["params"].get("path").is_none() || j["params"]["path"].is_null());
    assert!(
        j["params"].get("allow_insert_mode").is_none()
            || j["params"]["allow_insert_mode"].is_null()
    );
}

#[test]
fn get_references_at_request_with_include_declaration() {
    let req = ControlRequest::GetReferencesAt {
        line: 5,
        column: 3,
        path: None,
        allow_insert_mode: None,
        include_declaration: Some(true),
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["params"]["include_declaration"], true);
}

#[test]
fn get_workspace_symbols_request() {
    let req = ControlRequest::GetWorkspaceSymbols { query: "main".into() };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-workspace-symbols");
    assert_eq!(j["params"]["query"], "main");
}

#[test]
fn hover_response_with_some_hover() {
    let resp = ControlResponse::GetHoverAt {
        hover: Some(LspHover {
            contents: "fn main()".into(),
            range: None,
        }),
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "get-hover-at");
    assert_eq!(j["result"]["hover"]["contents"], "fn main()");
}

#[test]
fn hover_response_with_none() {
    let resp = ControlResponse::GetHoverAt { hover: None };
    let j = serde_json::to_value(&resp).unwrap();
    assert!(j["result"]["hover"].is_null());
}

#[test]
fn definition_response_with_locations() {
    let resp = ControlResponse::GetDefinitionAt {
        locations: vec![LspLocation {
            path: "src/lib.rs".into(),
            path_abs: "/repo/src/lib.rs".into(),
            range: LspRange {
                start: LspPosition { line: 10, character: 0 },
                end: LspPosition { line: 12, character: 1 },
            },
        }],
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["result"]["locations"][0]["path"], "src/lib.rs");
}

#[test]
fn diagnostics_response_with_empty_list() {
    let resp = ControlResponse::GetDiagnostics { diagnostics: vec![] };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "get-diagnostics");
    assert_eq!(j["result"]["diagnostics"], serde_json::json!([]));
}

#[test]
fn workspace_symbols_response_round_trips() {
    let resp = ControlResponse::GetWorkspaceSymbols {
        symbols: vec![LspSymbolInfo {
            name: "main".into(),
            kind: "function".into(),
            location: LspLocation {
                path: "src/main.rs".into(),
                path_abs: "/repo/src/main.rs".into(),
                range: LspRange {
                    start: LspPosition { line: 0, character: 3 },
                    end: LspPosition { line: 0, character: 7 },
                },
            },
            container_name: None,
        }],
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["result"]["symbols"][0]["kind"], "function");
}

#[test]
fn format_document_request_with_optional_path() {
    let req = ControlRequest::FormatDocument { path: Some("src/main.rs".into()) };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "format-document");
    assert_eq!(j["params"]["path"], "src/main.rs");
}

#[test]
fn format_document_request_with_no_path_omits_field() {
    let req = ControlRequest::FormatDocument { path: None };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "format-document");
    assert!(j["params"].get("path").is_none() || j["params"]["path"].is_null());
}

#[test]
fn format_document_response_round_trips() {
    let resp = ControlResponse::FormatDocument { applied: true };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "format-document");
    assert_eq!(j["result"]["applied"], true);
    let back: ControlResponse = serde_json::from_value(j).unwrap();
    let ControlResponse::FormatDocument { applied } = back else {
        panic!("wrong variant");
    };
    assert!(applied);
}

#[test]
fn run_command_request_with_no_args_omits_field() {
    let req = ControlRequest::RunCommand { name: "write".into(), args: vec![] };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "run-command");
    assert_eq!(j["params"]["name"], "write");
    assert!(j["params"].get("args").is_none() || j["params"]["args"].as_array().map(|a| a.is_empty()).unwrap_or(true));
}

#[test]
fn run_command_request_with_args() {
    let req = ControlRequest::RunCommand {
        name: "open".into(),
        args: vec!["src/main.rs".into()],
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["params"]["args"][0], "src/main.rs");
}

#[test]
fn run_command_response_with_message_round_trips() {
    let resp = ControlResponse::RunCommand {
        message: Some("written 5 buffers".into()),
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["result"]["message"], "written 5 buffers");
}

#[test]
fn run_command_response_with_no_message_omits_field() {
    let resp = ControlResponse::RunCommand { message: None };
    let j = serde_json::to_value(&resp).unwrap();
    assert!(j["result"].get("message").is_none() || j["result"]["message"].is_null());
}
