//! `serve` subcommand: stdio MCP server.
//!
//! Reads MCP protocol on stdin, writes responses on stdout. stderr is
//! reserved for logs (env_logger). Run by Claude Code via the `.mcp.json`
//! config in a project workspace.

use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    ServerHandler,
    model::{
        AnnotateAble, CallToolRequestParams, CallToolResult, Content, ErrorCode, Implementation,
        InitializeResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams, RawResource,
        ReadResourceRequestParams, ReadResourceResult, ResourceContents, ServerCapabilities,
        ServerInfo, Tool,
    },
    service::{RequestContext, RoleServer},
};

use crate::resources::{self, ResourceKind};
use crate::tools::ToolKind;

struct HelixMcpServer;

impl ServerHandler for HelixMcpServer {
    fn get_info(&self) -> ServerInfo {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_tools()
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

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let tools: Vec<Tool> = ToolKind::all()
            .map(|k| {
                Tool::new(
                    k.name(),
                    k.description(),
                    Arc::new(
                        k.input_schema()
                            .as_object()
                            .expect("input_schema() always returns an object")
                            .clone(),
                    ),
                )
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        params: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        use crate::tools::*;
        use helix_context_schema::ControlRequest;

        let name = params.name.as_ref();
        let args = params.arguments.unwrap_or_default();
        let args_val = serde_json::Value::Object(args);

        let kind = match ToolKind::from_name(name) {
            Some(k) => k,
            None => return Ok(tool_error(format!("Unknown tool: {}", name))),
        };

        let request = match kind {
            ToolKind::HelixOpenFile => {
                match serde_json::from_value::<HelixOpenFileArgs>(args_val) {
                    Ok(a) => ControlRequest::OpenFile { path: a.path },
                    Err(e) => {
                        return Ok(tool_error(format!(
                            "Invalid arguments for helix_open_file: {}",
                            e
                        )))
                    }
                }
            }
            ToolKind::HelixGotoLine => {
                match serde_json::from_value::<HelixGotoLineArgs>(args_val) {
                    Ok(a) => ControlRequest::GotoLine {
                        line: a.line,
                        column: a.column,
                        path: a.path,
                    },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_goto_line: {}", e))),
                }
            }
            ToolKind::HelixGetDiagnostics => {
                match serde_json::from_value::<HelixGetDiagnosticsArgs>(args_val) {
                    Ok(a) => ControlRequest::GetDiagnostics { path: a.path },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_diagnostics: {}", e))),
                }
            }
            // The other four variants land in tasks 5-6 — bail for now.
            _ => {
                return Ok(tool_error(format!(
                    "Tool {} not yet implemented (Phase 4b in progress)",
                    name
                )))
            }
        };

        Ok(dispatch_tool(request).await)
    }
}

pub async fn run() -> Result<()> {
    log::info!("helix-claude-mcp serve starting");

    let transport = rmcp::transport::stdio();
    let service = rmcp::serve_server(HelixMcpServer, transport).await?;
    service.waiting().await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// dispatch_tool — discovery + RPC roundtrip adapter
// ---------------------------------------------------------------------------

use helix_context_schema::{ControlRequest, ControlResponse};

/// Discover the Helix socket, send `request`, format the response as an
/// MCP tool result. Pure adapter between Phase 4a's plumbing and rmcp's
/// tool-result type.
pub async fn dispatch_tool(request: ControlRequest) -> CallToolResult {
    use crate::{discovery, rpc_client};

    let socket = match discovery::find_helix_socket(None).await {
        Ok(s) => s,
        Err(e) => {
            return tool_error(format!(
                "Helix is not running in this workspace (no live control socket found): {}. \
                 Start Helix with [editor.control-socket] enabled = true.",
                e,
            ))
        }
    };

    match rpc_client::send_request(&socket, &request).await {
        Ok(resp) => format_response_as_tool_result(resp),
        Err(rpc_client::RpcError::HelixError(je)) => tool_error(format!(
            "Helix rejected the request: {} (code {})",
            je.message,
            je.code as i32,
        )),
        Err(e) => tool_error(format!("Failed to communicate with Helix: {}", e)),
    }
}

pub fn tool_error(message: String) -> CallToolResult {
    CallToolResult::error(vec![Content::text(message)])
}

fn format_response_as_tool_result(resp: ControlResponse) -> CallToolResult {
    let text = match resp {
        ControlResponse::Ok {} => "{\"ok\":true}".to_string(),
        other => serde_json::to_string(&other)
            .unwrap_or_else(|_| "{\"error\":\"serialization failed\"}".to_string()),
    };
    CallToolResult::success(vec![Content::text(text)])
}

