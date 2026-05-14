//! JSON schema for the Helix context snapshot file (`<workspace>/.helix/context.json`).
//!
//! Used by Helix itself (`helix-term::context_logger`) and by external tools that
//! consume the file (e.g. the planned `helix-mcp` bridge). Schema changes
//! happen here once and surface as compile errors on both producers and consumers.

mod protocol;
mod protocol_error;
mod snapshot;
mod source;
mod types;

pub use protocol::{
    ClientInfo, ControlRequest, ControlResponse, LineRange, LspDiagnostic, LspHover,
    LspLocation, LspPosition, LspRange, LspSymbolInfo, ServerCapabilities, ServerInfo,
};
pub use protocol_error::{JsonRpcError, JsonRpcErrorCode};
pub use snapshot::ContextSnapshot;
pub use source::UpdateSource;
pub use types::{Active, Cursor, Instance, OpenBuffer, Position, Selection};

pub const SCHEMA_VERSION: u32 = 2;
pub const MIN_SUPPORTED_READER: u32 = 1;
/// Helix control protocol version negotiated during the `initialize` handshake.
///
/// Format is `MAJOR.MINOR`. Major-version mismatches are rejected during
/// initialize (see spec §6.1). Bumping the major version is a breaking
/// wire-format change; minor bumps signal additive method extensions that
/// older clients can still use safely.
pub const PROTOCOL_VERSION: &str = "1.0";
