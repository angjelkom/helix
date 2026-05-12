//! Request → response dispatch. For Phase 2a, only the `initialize` method
//! is implemented; subsequent phases extend the match.

use helix_context_schema::{
    ClientInfo, ControlRequest, ControlResponse, JsonRpcError, JsonRpcErrorCode,
    ServerCapabilities, ServerInfo, PROTOCOL_VERSION,
};

/// Methods that don't require entering the editor event loop (no
/// `&mut Editor`). Currently just `initialize`. Returning `Ok(Some(resp))`
/// means we handled it inline; `Ok(None)` means it must be forwarded to
/// the main loop via `EditorEvent::ControlRequest`.
pub fn try_dispatch_inline(
    request: &ControlRequest,
) -> Option<Result<ControlResponse, JsonRpcError>> {
    match request {
        ControlRequest::Initialize {
            protocol_version,
            client_info,
        } => Some(handle_initialize(protocol_version, client_info)),
    }
}

fn handle_initialize(
    client_protocol_version: &str,
    _client_info: &ClientInfo,
) -> Result<ControlResponse, JsonRpcError> {
    if !is_compatible_protocol(client_protocol_version, PROTOCOL_VERSION) {
        return Err(JsonRpcError {
            code: JsonRpcErrorCode::InvalidParams,
            message: format!(
                "client protocol version {} is incompatible with server {}",
                client_protocol_version, PROTOCOL_VERSION
            ),
            data: None,
        });
    }
    Ok(ControlResponse::Initialize {
        protocol_version: PROTOCOL_VERSION.into(),
        helix_version: env!("CARGO_PKG_VERSION").into(),
        server_info: ServerInfo {
            name: "helix".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        capabilities: ServerCapabilities {
            read_methods: vec!["initialize".into()],
            write_methods: vec![],
        },
    })
}

/// Same major version means compatible. "1.0" ↔ "1.5" OK; "1.0" ↔ "2.0" not.
fn is_compatible_protocol(client: &str, server: &str) -> bool {
    let major = |s: &str| -> Option<u32> { s.split('.').next()?.parse().ok() };
    match (major(client), major(server)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_compatible_version_returns_ok() {
        let req = ControlRequest::Initialize {
            protocol_version: "1.0".into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let resp = try_dispatch_inline(&req).unwrap().unwrap();
        let ControlResponse::Initialize { capabilities, .. } = resp;
        assert!(capabilities.read_methods.contains(&"initialize".to_string()));
    }

    #[test]
    fn initialize_incompatible_major_version_returns_invalid_params() {
        let req = ControlRequest::Initialize {
            protocol_version: "2.0".into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let err = try_dispatch_inline(&req).unwrap().unwrap_err();
        assert_eq!(err.code, JsonRpcErrorCode::InvalidParams);
    }

    #[test]
    fn major_version_compatibility_check() {
        assert!(is_compatible_protocol("1.0", "1.0"));
        assert!(is_compatible_protocol("1.5", "1.0"));
        assert!(is_compatible_protocol("1.0", "1.5"));
        assert!(!is_compatible_protocol("2.0", "1.0"));
        assert!(!is_compatible_protocol("", "1.0"));
        assert!(!is_compatible_protocol("garbage", "1.0"));
    }
}
