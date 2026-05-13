//! `serve` subcommand: stdio MCP server.
//!
//! Reads MCP protocol on stdin, writes responses on stdout. stderr is
//! reserved for logs (env_logger). Run by Claude Code via the `.mcp.json`
//! config in a project workspace.

use anyhow::Result;
use rmcp::{
    ServerHandler,
    model::{
        AnnotateAble, ErrorCode, Implementation, InitializeResult, ListResourcesResult,
        PaginatedRequestParams, RawResource, ReadResourceRequestParams, ReadResourceResult,
        ResourceContents, ServerCapabilities, ServerInfo,
    },
    service::{RequestContext, RoleServer},
};

use crate::resources::{self, ResourceKind};

struct HelixMcpServer;

impl ServerHandler for HelixMcpServer {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        ))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let resource_list = ResourceKind::all()
            .map(|kind| {
                RawResource::new(kind.uri(), kind.name())
                    .with_description(kind.description())
                    .with_mime_type("application/json")
                    .no_annotation()
            })
            .collect::<Vec<_>>();

        Ok(ListResourcesResult::with_all_items(resource_list))
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let kind = match params.uri.as_str() {
            u if u == ResourceKind::Current.uri() => ResourceKind::Current,
            u if u == ResourceKind::Buffers.uri() => ResourceKind::Buffers,
            u if u == ResourceKind::Snapshot.uri() => ResourceKind::Snapshot,
            other => {
                return Err(rmcp::ErrorData::new(
                    ErrorCode::METHOD_NOT_FOUND,
                    format!("unknown resource URI: {}", other),
                    None,
                ));
            }
        };

        let workspace = resources::resolve_workspace(None)
            .map_err(|e| {
                rmcp::ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("failed to resolve workspace: {}", e),
                    None,
                )
            })?;

        let body = resources::read_resource(kind, &workspace);
        let uri = params.uri.clone();

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(body, uri).with_mime_type("application/json"),
        ]))
    }
}

pub async fn run() -> Result<()> {
    log::info!("helix-claude-mcp serve starting");

    let transport = rmcp::transport::stdio();
    let service = rmcp::serve_server(HelixMcpServer, transport).await?;
    service.waiting().await?;

    Ok(())
}
