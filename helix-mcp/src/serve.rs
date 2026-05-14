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

/// Returned in `InitializeResult.instructions`. MCP clients feed this to
/// the LLM as part of its system context on every session that loads
/// this server, so every coding agent (Claude Code, Codex, Cursor,
/// Cline, Continue, Zed, …) gets the same operating manual without
/// needing per-agent rules files.
///
/// Keep it tight: enough that the model knows what's available and the
/// right workflow, short enough that it doesn't dominate the system
/// prompt. Edit this when tool behavior changes.
const SERVER_INSTRUCTIONS: &str = r#"You are paired with a running Helix editor via this MCP server. Use it to keep the user's editor in sync with your work so they can follow along visually.

# Current editor state — two paths

Depending on how the client is configured, you may receive the user's current editor state in one of two ways. Both surface the same data; pick whichever is available:

1. **Inline injection** (Claude Code with the UserPromptSubmit hook, or any client that pre-injects context). The current snapshot appears in the user's prompt as a `<helix-editor-context>…</helix-editor-context>` block. When present, trust it as the primary source — no tool call needed.
2. **On-demand resource read**. When the inline block is absent, or when you need to confirm state after a tool call that may have changed it (open-file, goto-line, select, run-command, format-document), read `helix://state/current` via `resources/read`.

When the user says "this file" / "the file I'm editing" / "here" without naming a path, resolve it from whichever path is available rather than asking.

# Resources (read these for context, no side effects)

- `helix://state/current` — active buffer's path, cursor, selection, mode.
- `helix://state/buffers` — list of all open buffers.
- `helix://state/snapshot` — the full editor snapshot (everything above plus timestamps and instance info).

# Workflow: navigate before editing

Before calling Edit / Write on a file, navigate Helix to the change site so the user sees where the edit will land:

- Single-point edit (one-line change, insertion): call `helix_open_file` with `path`, `line`, and `column` set to where the edit will start. The view recenters on the target.
- Multi-line range replacement: call `helix_open_file` with the start line, then `helix_select` with the exact `(start_line, start_column, end_line, end_column)` range being replaced. The highlighted selection shows the user the about-to-change region.
- New file (Write to a nonexistent path): skip the pre-navigation, then call `helix_open_file` with the new path after the write so the file lands in Helix.

After the edit, call `helix_goto_line` (or `helix_open_file` with the new line) so the cursor stays on the change for follow-up.

Skip navigation only when: the file is outside the workspace, the bridge is down (a tool call returned "Helix is not running in this workspace" — don't retry), or the work is purely terminal-side.

# Tools

- `helix_open_file(path, line?, column?)` — open and optionally jump-and-center. Path may be absolute or workspace-relative.
- `helix_goto_line(line, column?, path?)` — move cursor; view recenters on the line.
- `helix_select(start_line, start_column, end_line, end_column, path?)` — select a range; view recenters on the head. 1-indexed inclusive.
- `helix_get_diagnostics(path?)` — LSP diagnostics for a buffer. Cheaper than running a separate type-check command.
- `helix_get_hover(line, column, path?)` — LSP hover info.
- `helix_get_definition(line, column, path?)` — LSP goto-definition.
- `helix_get_references(line, column, path?, include_declaration?)` — LSP find-references.
- `helix_get_workspace_symbols(query)` — LSP fuzzy symbol search across the workspace. Prefer this over grep when you want a symbol, not a string.
- `helix_format_document(path?)` — kick off the LSP formatter. Returns `applied: true` immediately; the edits arrive asynchronously via the LSP.
- `helix_run_command(name, args)` — execute any Helix typable command (without the leading colon). POWERFUL — can write files, run shell commands, quit the editor. Use only for things the user has explicitly asked for, e.g. `{name: "write"}` to save or `{name: "reload"}` to reload from disk.

# Insert-mode safety

`helix_get_hover` / `helix_get_definition` / `helix_get_references` refuse with error code -32003 (`BufferModeUnsafe`) when the editor is in Insert mode — querying mid-typing positions returns garbage. If you specifically need to override (rare), pass `allow_insert_mode: true`.

# Error handling

Tool errors include a structured message and an error code. "Helix is not running in this workspace" means the user doesn't have Helix open here — degrade gracefully and don't keep calling Helix tools that session. Resources still work when Helix is closed, serving the last-written snapshot from disk."#;

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
        .with_instructions(SERVER_INSTRUCTIONS)
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
                    Ok(a) => ControlRequest::OpenFile {
                        path: a.path,
                        line: a.line,
                        column: a.column,
                    },
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
            ToolKind::HelixSelect => {
                match serde_json::from_value::<HelixSelectArgs>(args_val) {
                    Ok(a) => ControlRequest::SelectRange {
                        start_line: a.start_line,
                        start_column: a.start_column,
                        end_line: a.end_line,
                        end_column: a.end_column,
                        path: a.path,
                    },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_select: {}", e))),
                }
            }
            ToolKind::HelixGetDiagnostics => {
                match serde_json::from_value::<HelixGetDiagnosticsArgs>(args_val) {
                    Ok(a) => ControlRequest::GetDiagnostics { path: a.path },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_diagnostics: {}", e))),
                }
            }
            ToolKind::HelixGetHover => {
                match serde_json::from_value::<HelixPositionArgs>(args_val) {
                    Ok(a) => ControlRequest::GetHoverAt {
                        line: a.line,
                        column: a.column,
                        path: a.path,
                        allow_insert_mode: a.allow_insert_mode,
                    },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_hover: {}", e))),
                }
            }
            ToolKind::HelixGetDefinition => {
                match serde_json::from_value::<HelixPositionArgs>(args_val) {
                    Ok(a) => ControlRequest::GetDefinitionAt {
                        line: a.line,
                        column: a.column,
                        path: a.path,
                        allow_insert_mode: a.allow_insert_mode,
                    },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_definition: {}", e))),
                }
            }
            ToolKind::HelixGetReferences => {
                match serde_json::from_value::<HelixGetReferencesArgs>(args_val) {
                    Ok(a) => ControlRequest::GetReferencesAt {
                        line: a.line,
                        column: a.column,
                        path: a.path,
                        allow_insert_mode: a.allow_insert_mode,
                        include_declaration: a.include_declaration,
                    },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_references: {}", e))),
                }
            }
            ToolKind::HelixGetWorkspaceSymbols => {
                match serde_json::from_value::<HelixGetWorkspaceSymbolsArgs>(args_val) {
                    Ok(a) => ControlRequest::GetWorkspaceSymbols { query: a.query },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_get_workspace_symbols: {}", e))),
                }
            }
            ToolKind::HelixFormatDocument => {
                match serde_json::from_value::<HelixFormatDocumentArgs>(args_val) {
                    Ok(a) => ControlRequest::FormatDocument { path: a.path },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_format_document: {}", e))),
                }
            }
            ToolKind::HelixRunCommand => {
                match serde_json::from_value::<HelixRunCommandArgs>(args_val) {
                    Ok(a) => ControlRequest::RunCommand {
                        name: a.name,
                        args: a.args,
                    },
                    Err(e) => return Ok(tool_error(format!("Invalid arguments for helix_run_command: {}", e))),
                }
            }
        };

        Ok(dispatch_tool(request).await)
    }
}

pub async fn run() -> Result<()> {
    log::info!("helix-mcp serve starting");

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
async fn dispatch_tool(request: ControlRequest) -> CallToolResult {
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

fn tool_error(message: String) -> CallToolResult {
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

