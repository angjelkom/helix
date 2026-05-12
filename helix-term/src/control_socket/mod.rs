//! Unix-domain JSON-RPC control socket for external tools (e.g. the
//! helix-claude-mcp bridge). See spec §5-§6.

pub mod dispatch;
pub mod framing;
pub mod lifecycle;
pub mod path;
pub mod server;
