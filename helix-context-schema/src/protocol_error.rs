//! JSON-RPC 2.0 error types and codes used by the Helix control protocol.
//!
//! Standard codes follow https://www.jsonrpc.org/specification#error_object.
//! Helix-specific codes use the -32000..=-32099 server-error range.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// All error codes the Helix control socket may return. Serialized as the
/// underlying `i32` so wire format matches JSON-RPC 2.0 exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum JsonRpcErrorCode {
    // Standard JSON-RPC 2.0 codes
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,

    // Helix-specific (-32000 to -32099 is JSON-RPC's server-error range)
    NoLspForLanguage = -32001,
    LspTimeout = -32002,
    BufferModeUnsafe = -32003,
    NoActiveDocument = -32004,
    PathOutsideWorkspace = -32005,
}

impl Serialize for JsonRpcErrorCode {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_i32(*self as i32)
    }
}

impl<'de> Deserialize<'de> for JsonRpcErrorCode {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let n = i32::deserialize(de)?;
        match n {
            -32700 => Ok(Self::ParseError),
            -32600 => Ok(Self::InvalidRequest),
            -32601 => Ok(Self::MethodNotFound),
            -32602 => Ok(Self::InvalidParams),
            -32603 => Ok(Self::InternalError),
            -32001 => Ok(Self::NoLspForLanguage),
            -32002 => Ok(Self::LspTimeout),
            -32003 => Ok(Self::BufferModeUnsafe),
            -32004 => Ok(Self::NoActiveDocument),
            -32005 => Ok(Self::PathOutsideWorkspace),
            other => Err(serde::de::Error::custom(format!(
                "unknown JSON-RPC error code {}",
                other
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: JsonRpcErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<Value>,
}
