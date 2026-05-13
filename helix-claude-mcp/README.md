# helix-claude-mcp

MCP bridge that exposes Helix editor state and commands to Claude Code.

## What this is

A small Rust binary that bridges two pieces:

- **Helix's control socket** at `<workspace>/.helix/control-<pid>.sock` (a custom JSON-RPC dialect spoken by Helix's `[editor.control-socket]` feature). See `../docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md`.
- **Claude Code's MCP** (Model Context Protocol) over stdio.

Claude Code's `.mcp.json` configures this binary as a stdio MCP server. Once it's running, Claude can:

- Read editor state via MCP **Resources** (`helix://state/current`, `helix://state/buffers`, `helix://state/snapshot`).
- Drive the editor via MCP **Tools** (open files, jump to lines, query LSP).

## Installation

From the workspace root:

```bash
cargo build --release -p helix-claude-mcp
cp target/release/helix-claude-mcp ~/.cargo/bin/
```

Or run it directly from `target/release/helix-claude-mcp`.

## Claude Code configuration

Add this to your project's `.mcp.json` (or to your global `~/.claude.json` MCP servers list):

```json
{
  "mcpServers": {
    "helix": {
      "command": "helix-claude-mcp",
      "args": ["serve"]
    }
  }
}
```

Claude Code spawns the process per session and sets `CLAUDE_PROJECT_DIR` automatically.

## Helix configuration

In `~/.config/helix/config.toml`:

```toml
[editor.context-logger]
enabled = true

[editor.control-socket]
enabled = true
```

## Available Tools

Phase 4b shipped these tools. Claude Code can call any of them via MCP `tools/call`:

| Tool | What it does |
|---|---|
| `helix_open_file` | Open a file in Helix and focus it. |
| `helix_goto_line` | Move the cursor to a line/column. |
| `helix_get_diagnostics` | List LSP diagnostics for a file. |
| `helix_get_hover` | LSP hover info at a position. |
| `helix_get_definition` | LSP goto-definition. |
| `helix_get_references` | LSP find-references. |
| `helix_get_workspace_symbols` | LSP workspace symbol search. |

All tools require Helix to be running with `[editor.control-socket] enabled = true`. When Helix isn't running, tools return a clear "not running" error message.

## How it works

- **Resources** read from the snapshot file `<workspace>/.helix/context.json` — fast, no Helix process required (returns a friendly error if the snapshot is missing).
- **Tools** connect to the live Helix control socket via discovery — globbing `<workspace>/.helix/control-*.sock` and picking the live one.

## Subcommands

- `helix-claude-mcp serve` — stdio MCP server.
- `helix-claude-mcp hook` — UserPromptSubmit hook handler. *(Phase 5 — not yet implemented.)*
