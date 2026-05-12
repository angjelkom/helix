//! JSON-RPC 2.0 protocol types for the Helix control socket dialect.
//!
//! The wire format is *not* MCP — it's a small custom dialect specific to
//! Helix. An external bridge translates between this and MCP. See spec §6.

use serde::{Deserialize, Serialize};

/// Identification of the client connecting to Helix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Identification of the Helix server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// What this Helix instance can do for clients. The lists are method-name
/// strings (kebab-case, matching the JSON method tags).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub read_methods: Vec<String>,
    pub write_methods: Vec<String>,
}

/// All possible requests the control socket accepts. The wire format uses
/// JSON-RPC 2.0 with `method` and `params` keys; serde's `tag = "method"`
/// generates exactly that shape, and the variant name (kebab-cased) is the
/// method tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "kebab-case")]
pub enum ControlRequest {
    Initialize {
        protocol_version: String,
        client_info: ClientInfo,
    },
}

/// All possible successful responses. The variant name (kebab-cased) matches
/// the request that produced it. Wraps the result payload in a `result` key
/// to mirror JSON-RPC 2.0's response shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "kebab-case")]
pub enum ControlResponse {
    Initialize {
        protocol_version: String,
        helix_version: String,
        server_info: ServerInfo,
        capabilities: ServerCapabilities,
    },
}
