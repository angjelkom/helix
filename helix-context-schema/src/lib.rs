//! JSON schema for the Helix context snapshot file (`<workspace>/.helix/context.json`).
//!
//! Used by Helix itself (`helix-term::context_logger`) and by external tools that
//! consume the file (e.g. the planned `helix-claude-mcp` bridge). Schema changes
//! happen here once and surface as compile errors on both producers and consumers.

mod snapshot;
mod source;
mod types;

pub use snapshot::ContextSnapshot;
pub use source::UpdateSource;
pub use types::{Active, Cursor, Instance, OpenBuffer, Position, Selection};

pub const SCHEMA_VERSION: u32 = 2;
pub const MIN_SUPPORTED_READER: u32 = 1;
