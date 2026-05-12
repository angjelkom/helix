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
    let ControlRequest::Initialize { protocol_version, client_info } = req;
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
    let ControlResponse::Initialize { helix_version, .. } = back;
    assert_eq!(helix_version, "25.7.1");
}
