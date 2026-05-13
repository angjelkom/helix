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
- `helix-claude-mcp hook` — UserPromptSubmit hook handler (see below).

## Hook subcommand

`helix-claude-mcp hook` is the Rust replacement for the shell hook script at `~/.claude/hooks/helix-context.sh`. Same wire contract — reads Claude Code's hook payload on stdin, writes the wrapped snapshot to stdout (or nothing if skipped). Use it in two places:

### UserPromptSubmit

Inject the snapshot at the start of every prompt (skipped when already-injected or when the snapshot's `last_update_source: "mcp_command"`):

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          { "type": "command", "command": "helix-claude-mcp hook", "timeout": 5 }
        ]
      }
    ]
  }
}
```

### Compression-aware reset

When Claude Code compacts the context (auto or `/compact`), the previously-injected snapshot is gone. Clear the marker so the next prompt re-injects:

```json
{
  "hooks": {
    "PostCompact": [
      {
        "hooks": [
          { "type": "command", "command": "helix-claude-mcp hook --reset-marker", "timeout": 5 }
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "compact",
        "hooks": [
          { "type": "command", "command": "helix-claude-mcp hook --reset-marker", "timeout": 5 }
        ]
      }
    ]
  }
}
```

### How it dedupes

Marker file at `$XDG_RUNTIME_DIR/claude-helix/marker-${session_id}` (Linux) or `~/Library/Caches/claude-helix/marker-${session_id}` (macOS) holds the snapshot's mtime at last injection. On each call:

1. Parse stdin (must contain `session_id` and `cwd`; serde drops unknown fields).
2. Locate the snapshot at `$CLAUDE_PROJECT_DIR/.helix/context.json` (or walk up from `cwd`).
3. Skip if missing, > 24h stale, malformed, or `last_update_source == "mcp_command"`.
4. Skip if marker mtime matches snapshot mtime (already injected this session).
5. Otherwise: emit wrapped snapshot, then write snapshot mtime into the marker file.

Failure modes (stdin parse error, marker write failure, etc.) exit 0 silently — the hook is best-effort and never fails the user's prompt.

## Migrating from the shell hook

If you previously used the shell hook at `~/.claude/hooks/helix-context.sh`, replace your `~/.claude/settings.json` hooks block. The shell hook can be deleted after switching; nothing references it.

Old:
```json
{ "type": "command", "command": "/Users/you/.claude/hooks/helix-context.sh" }
```

New:
```json
{ "type": "command", "command": "helix-claude-mcp hook", "timeout": 5 }
```

The Rust hook is functionally a superset of the shell version: same emit format, plus proper per-session dedup (the shell version had none — it re-emitted on every prompt) and `--reset-marker` for compression.
