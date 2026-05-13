# Helix ↔ Claude Code Bridge — Design Spec

**Status:** Draft
**Date:** 2026-05-12
**Author:** Angjelko
**Supersedes:** Extends `helix-term/src/context_logger.rs` (shipped 2026-05-11)

## 1. Overview

We are building bidirectional integration between Helix (editor) and Claude Code (AI assistant) so that Claude can see the user's editor state and, when needed, drive the editor and query its LSP knowledge.

The integration has two complementary layers, intentionally combined rather than alternatives:

1. **Passive prefetch (already shipped):** Helix writes a JSON snapshot to a workspace file on terminal focus loss. Claude Code's `UserPromptSubmit` hook injects it into every relevant prompt. Speed floor (~1 ms). No protocol overhead. Claude knows what the user is editing without being asked.

2. **Active capability (this spec):** An external Rust binary speaks MCP to Claude Code and talks to Helix over a Unix-socket JSON-RPC dialect. Enables Claude to:
   - Read live state (current cursor, selection, mode)
   - Query Helix's LSP (hover, definition, references, diagnostics, symbols)
   - Drive the editor (open file, jump to line, run typable commands)

The custom JSON-RPC dialect — not MCP itself — lives inside Helix. The MCP protocol is implemented entirely in the external binary, decoupling Helix from MCP-spec evolution.

## 2. Goals and non-goals

### Goals

- Zero idle overhead on Helix when both `editor.context-logger.enabled` and `editor.control-socket.enabled` are `false` (default)
- Sub-millisecond context delivery for the common "user prompts about current code" workflow, via the file hook path (the MCP path is intentionally on-demand and pays roundtrip cost)
- Live LSP queries available to Claude on demand
- Bidirectional: Claude can take actions in Helix
- Terminal-multiplexer-agnostic (no Kitty/Tmux dependency)
- Multi-instance safe: user can run multiple Helix sessions in multiple projects
- Stable contract between Helix and the bridge — Helix's internal JSON-RPC dialect can evolve via versioning without breaking external clients

### Non-goals

- Native MCP implementation inside Helix core (rejected: too much code, ties Helix to MCP spec evolution)
- Upstream-ability to `helix-editor/helix` master (this is a fork feature)
- Steel-language MCP server (rejected: Steel runtime lacks networking, file I/O, stdio primitives)
- Helix-initiates-Claude direction (separate problem, separate design)
- Support for terminals or transports other than stdio MCP

## 3. Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                          Claude Code                                  │
│  ┌─────────────┐                            ┌────────────────────┐    │
│  │ prompt flow │──UserPromptSubmit hook────▶│ context.sh OR      │    │
│  │             │                            │ helix-claude-mcp   │    │
│  │             │                            │   hook             │    │
│  │             │◀───reads .helix/context.json (passive prefetch)─│    │
│  │             │                            │                    │    │
│  │             │◀──MCP (stdio) Tools+Resources──┐                │    │
│  └─────────────┘                                │                │    │
└─────────────────────────────────────────────────┼────────────────┘    │
                                                  │                     │
                                ┌─────────────────▼──────────────────┐  │
                                │ helix-claude-mcp serve             │  │
                                │  (external Rust binary, stdio MCP) │  │
                                └─────────────────┬──────────────────┘  │
                                                  │                     │
                                                  │ Unix socket          │
                                                  │ JSON-RPC 2.0         │
                                                  │ (custom dialect)     │
                                                  ▼                     │
            ┌─────────────────────────────────────────────────────┐    │
            │                       Helix                           │    │
            │  ┌──────────────────┐    ┌───────────────────────┐   │    │
            │  │ control socket   │    │ context_logger        │   │    │
            │  │ (new in this     │◀──▶│ (existing)            │   │    │
            │  │  spec)           │    │ writes snapshot       │   │    │
            │  └────────┬─────────┘    └───────────────────────┘   │    │
            │           │                                            │    │
            │           ▼                                            │    │
            │  ┌──────────────────────────────────────────────┐     │    │
            │  │     event_loop tokio::select! (main thread)  │     │    │
            │  │ - terminal input                              │     │    │
            │  │ - signals                                     │     │    │
            │  │ - job callbacks                               │     │    │
            │  │ - control requests  ◀─── NEW BRANCH           │     │    │
            │  │ - editor.wait_event                           │     │    │
            │  └──────────────────────────────────────────────┘     │    │
            └──────────────────────────────────────────────────────┘    │
                                                                         │
            ┌──────────────────────────────────────────────────────┐    │
            │       Shared crate: helix-context-schema             │────┘
            │       (types used by both Helix and helix-claude-mcp)│
            └──────────────────────────────────────────────────────┘
```

### Components

- **`helix-context-schema`** (new shared crate): the JSON snapshot types + JSON-RPC method definitions. Depended on by both `helix-term` and `helix-claude-mcp`.
- **Helix-side changes** (in `helix-term`):
  - Control socket: Unix-domain server at `<workspace>/.helix/control-<pid>.sock` (with macOS path-length fallback — see §5.2)
  - Editor-event channel: `EditorEvent::ControlRequest` variant carrying request + `oneshot` reply sender
  - Snapshot extension: add `last_update_source` field (always set); optional `instance` block (PID + socket_path hint, informational only)
  - Workspace fallback fix: when `find_workspace` returns `is_cwd_fallback=true`, skip writing the snapshot (don't pollute `$HOME/.helix/`)
  - No global instance registry. No startup directory scan. Discovery is project-local.
- **`helix-claude-mcp` binary** (new repo or workspace member):
  - `serve` subcommand: stdio MCP server, discovers Helix by globbing `$CLAUDE_PROJECT_DIR/.helix/control-*.sock` and connecting to whichever responds (§7.4)
  - `hook` subcommand: UserPromptSubmit handler with dedup logic (replaces shell hook eventually)
  - `--reset-marker` flag: for `PostCompact`/`SessionStart compact` hooks
- **Claude Code integration:**
  - Project-scoped `.mcp.json` configures the MCP server
  - `~/.claude/settings.json` hooks point at the same binary

## 4. Snapshot schema v2

Bumping `schema_version` from 1 to 2. v2 adds fields; v1 readers tolerate unknown keys (already verified) but a v2-only writer signals intent.

```jsonc
{
  "schema_version": 2,
  "min_supported_reader": 1,
  "timestamp": "2026-05-12T14:32:01Z",
  "last_update_source": "focus_lost",   // | "mcp_command" | "manual"
  "instance": {                         // optional; informational only
    "pid": 12345,
    "socket_path": "/Users/angm/repo/.helix/control-12345.sock",
    "started_at": "2026-05-12T10:00:00Z"
  },
  "project_root": "/Users/angm/repo",
  "mode": "normal",
  "active": {
    "path": "src/main.rs",
    "path_abs": "/Users/angm/repo/src/main.rs",
    "language": "rust",
    "modified": false,
    "line_count": 200,
    "cursors": [
      { "primary": true, "line": 17, "column": 5 }
    ],
    "selections": []
  },
  "open_buffers": [
    { "path": "src/lib.rs", "language": "rust", "modified": false }
  ]
}
```

The `instance` block is a discovery hint, not a contract. The MCP server doesn't depend on it — discovery works by globbing `<workspace>/.helix/control-*.sock` (see §7.4). The block is included for diagnostic tools and future use.

When `find_workspace()` returns `is_cwd_fallback=true` (Helix launched from a directory with no `.git`/`.svn`/`.jj`/`.helix` marker, e.g. `$HOME`), the snapshot is **not written**. This avoids polluting unrelated directories. Logged at debug level only.

Removed from v1 (move to MCP on-demand): nothing currently, but explicitly **do not add** diagnostics, LSP info, project tree, or full buffer text — those flow through MCP.

## 5. Helix-side: control socket

### 5.1 Configuration

Extend `[editor.context-logger]` in config.toml:

```toml
[editor.context-logger]
enabled = true                              # writes snapshot file
include-selection-text = true
include-buffer-text = false
max-selection-bytes = 8192
# Path stays config-driven for power users; default resolves at runtime
path = ".helix/context.json"

[editor.control-socket]
enabled = false                             # opt-in
path = ""                                   # empty = use <workspace>/.helix/control-<pid>.sock
```

When `control-socket.enabled = true`, Helix:
1. Computes socket path (see §5.2)
2. Checks for an orphaned socket at our own path and unlinks if dead (§5.3)
3. Binds a `tokio::net::UnixListener` and applies `chmod 0600`
4. Adds a `select!` branch to the event loop
5. Cleans up on shutdown (close + unlink own socket)

When `enabled = false` (default): zero work, zero overhead, identical behavior to today.

### 5.2 Socket path resolution

In order:
1. `editor.control-socket.path` if non-empty (explicit override)
2. `<workspace>/.helix/control-<pid>.sock` — the default for almost all users
3. **macOS path-length fallback:** if the resolved path from (2) exceeds the platform's `sun_path` limit (104 bytes on macOS, 108 on Linux), fall back to `$XDG_RUNTIME_DIR/helix/control-<pid>-<workspace_hash>.sock` (or `$TMPDIR/...` on macOS) and write a small pointer file at `<workspace>/.helix/control-<pid>.sock.path` containing the real path. MCP-server discovery checks for `*.sock.path` pointers first.

**Permissions race avoidance:** there is a window between `bind()` and a post-bind `chmod()` during which the socket inherits the process umask (typically `0o022` on most systems → 0644 default mode). On multi-user machines this is a real exposure. The implementation should:

1. Set `umask(0o077)` before `bind()` (POSIX `umask(2)` is per-process; capture the old value first).
2. `bind()`.
3. Restore the prior umask.
4. As belt-and-suspenders, `chmod(socket_path, 0o600)` post-bind.

The umask change makes the socket mode 0600 atomically with creation; the explicit chmod is redundant correctness insurance.

Worktree behavior: each worktree has its own root (its `.git` file satisfies `find_workspace`'s `.exists()` check at `helix-loader/src/lib.rs:273`), so two worktrees of the same repo get two distinct sockets in two distinct `.helix/` directories. No special handling required.

### 5.3 Socket lifecycle

**On startup:**
1. Compute `our_socket_path` per §5.2.
2. If `our_socket_path` exists, attempt `connect()` to it. If `ECONNREFUSED`, the socket is orphaned from a prior crash — unlink it.
3. Bind, then `chmod 0600`.

Total cost: three syscalls. No directory scan, no loop over peer sockets. Sub-millisecond, invisible against Helix's existing startup cost (theme load, grammar discovery, runtime scan).

**On graceful shutdown** (in `Application::close`, `helix-term/src/application.rs:1437`):
1. Drop the `UnixListener`.
2. `unlink` our own socket file (and pointer file, if used).

**On crash (SIGKILL, panic, etc.):**
The socket file is left behind. This is harmless:
- A future Helix instance with the same PID (rare) will detect and unlink it per the startup pattern above.
- The MCP server's discovery filters out unconnectable sockets (see §7.4), so stale files cause no functional issue.

**Optional hygiene task** (post-startup, deferred):
```rust
tokio::spawn(async move {
    tokio::time::sleep(Duration::from_secs(5)).await;
    cleanup_stale_sibling_sockets(&workspace).await;
});
```
Runs on a worker thread, after UI is responsive. Glob `.helix/control-*.sock`, try to connect to each, unlink the dead ones. **Not on the critical path.** Can be omitted entirely if simplicity is preferred; the litter accumulates only across crashes and is functionally inert.

### 5.4 Event loop integration

Add to `EditorEvent` (defined at `helix-view/src/editor.rs:1428`, currently derives `Debug` only):

```rust
pub enum EditorEvent {
    // existing variants: DocumentSaved, ConfigEvent, LanguageServerMessage,
    // DebuggerEvent, IdleTimer, Redraw
    ControlRequest {
        request: ControlRequest,
        reply: tokio::sync::oneshot::Sender<ControlResponse>,
    },
}
```

The variant carries a `oneshot::Sender` which does not implement `Debug`. Solution: hand-write `impl Debug for EditorEvent` — the existing `#[derive(Debug)]` is dropped, all six existing variants render the same way they do today (verified by `dbg!` output), and the new `ControlRequest` variant renders as `ControlRequest { request: ..., reply: <oneshot::Sender> }`. No new external dependency (don't pull in `derivative` or `educe` just for this).

In `helix-term/src/application.rs::event_loop_until_idle`, add a new arm to `tokio::select!` (after `signal`, before `input_stream` — high priority but doesn't preempt OS signals):

```rust
Some(EditorEvent::ControlRequest { request, reply }) = self.control_rx.recv() => {
    self.handle_control_request(request, reply).await;
}
```

The handler runs on the main task and has `&mut self.editor`, `&mut self.compositor`, etc. — full editor access. For most methods it can satisfy the request synchronously and send the reply.

For LSP-backed methods (hover, definition, references), the handler:
1. Constructs the LSP future via the existing `language_servers_with_feature` machinery
2. Returns a request token to the socket task
3. The socket task awaits the LSP future directly (LSP client handles are `Send + Clone`)
4. The socket task replies once the LSP returns

This way LSP latency doesn't freeze the editor; the editor only spends the few microseconds needed to construct the future.

### 5.5 Snapshot rewrite on state mutation

After any control method that mutates editor state (open-file, goto-line, run-command), the handler calls `context_logger::write_context_file(editor, Source::McpCommand)` **directly** — not via `helix_event::dispatch(TerminalFocusLost)`. Direct call avoids firing Steel hooks spuriously.

Pure read methods (current-state, get-hover-at, get-diagnostics) do not rewrite the snapshot.

## 6. JSON-RPC method surface (Helix control socket)

JSON-RPC 2.0 over a single-newline-delimited stream per connection. Each connection is one client (typically `helix-claude-mcp serve`).

### 6.1 Lifecycle

- `initialize` — handshake. Client sends `{protocolVersion: "1.0", clientInfo: {name, version}}`; Helix returns `{protocolVersion, helixVersion, capabilities: {...}}`. Reject mismatched majors.
- `shutdown` — graceful close request (server cleans up next connection accept).

### 6.2 Read methods (no state mutation)

| Method | Params | Returns |
|---|---|---|
| `current-state` | `{}` | Same shape as snapshot's `active` field |
| `get-buffer-text` | `{buffer_id?: int, path?: string, range?: {start_line, end_line}}` | `{text, language}` |
| `get-diagnostics` | `{path?: string}` (defaults to current file) | `{diagnostics: [...]}` |
| `get-hover-at` | `{line, column, path?}` | `{contents: string \| null}` (LSP-backed) |
| `get-definition-at` | `{line, column, path?}` | `{locations: [{path, line, column}]}` (LSP-backed) |
| `get-references-at` | `{line, column, path?}` | `{locations: [{path, line, column, context_line}]}` (LSP-backed) |
| `get-workspace-symbols` | `{query: string}` | `{symbols: [{name, kind, path, line}]}` (LSP-backed) |
| `get-open-buffers` | `{}` | `{buffers: [...]}` |

### 6.3 Write methods (mutate state, trigger snapshot rewrite)

| Method | Params | Returns |
|---|---|---|
| `open-file` | `{path, line?, column?}` | `{ok: true}` |
| `goto-line` | `{line, column?, path?}` | `{ok: true}` |
| `run-command` | `{name, args: []}` | `{ok: bool, output?: string}` |
| `format-document` | `{path?}` | `{applied: bool}` |

### 6.4 Error handling

Standard JSON-RPC error codes:
- `-32700` ParseError
- `-32600` InvalidRequest
- `-32601` MethodNotFound
- `-32602` InvalidParams
- `-32603` InternalError

Custom range `-32000` to `-32099`:
- `-32001` NoLspForLanguage
- `-32002` LspTimeout
- `-32003` BufferModeUnsafe (e.g., LSP query while in insert mode mid-typing)
- `-32004` NoActiveDocument
- `-32005` PathOutsideWorkspace

### 6.5 Mid-typing safety

LSP query methods (`get-hover-at`, `get-definition-at`, `get-references-at`) optionally take `allow_insert_mode: bool` (default `false`). When `false` and the editor is currently in `Mode::Insert`, the method returns error `-32003` BufferModeUnsafe. This prevents querying garbage positions mid-keystroke.

**Note:** an earlier draft of this spec specified a "last-keystroke time < 500 ms" heuristic. That heuristic referenced a field that does not exist in `Editor`. Implementing such a heuristic would require adding an `Instant` field to `Editor` updated on every input event — non-trivial and arguably premature. Initial implementation refuses on `Mode::Insert` alone. If false positives prove problematic (e.g., user pauses in insert mode and wants hover), introduce the timestamp field as a follow-up.

## 7. MCP server binary (`helix-claude-mcp`)

### 7.1 Crate organization

A new workspace member at `helix-claude-mcp/` containing:

```
helix-claude-mcp/
├── Cargo.toml
├── src/
│   ├── main.rs        # CLI: subcommand dispatch
│   ├── serve.rs       # MCP stdio server
│   ├── hook.rs        # UserPromptSubmit hook
│   ├── discovery.rs   # Find Helix instance for current workspace
│   ├── rpc_client.rs  # JSON-RPC client over Unix socket
│   └── mcp_impl.rs    # MCP Resources & Tools handlers
```

Dependencies:
- `rmcp` (official Rust MCP SDK, `modelcontextprotocol/rust-sdk`)
- `tokio`, `serde`, `serde_json`, `anyhow`, `clap`
- `helix-context-schema` (shared crate, workspace path dep)

### 7.2 Subcommands

```
helix-claude-mcp serve              # stdio MCP server
helix-claude-mcp hook [--reset-marker]
                                    # UserPromptSubmit / PostCompact / SessionStart hook
```

The `hook` subcommand reads JSON from stdin (Claude Code's hook payload), uses `CLAUDE_PROJECT_DIR` env var to find the right snapshot, applies dedup, prints to stdout (or skips silently).

### 7.3 MCP Resources vs Tools mapping

Per convention (Resources = static state, Tools = LLM-initiated actions/queries):

**Resources** (Claude reads via `resources/read`):
- `helix://state/current` — current cursor, selection, file, mode
- `helix://state/buffers` — list of open buffers
- `helix://state/snapshot` — same as the .helix/context.json content

**Tools** (Claude calls via `tools/call`):
- `helix_open_file(path, line?, column?)`
- `helix_goto_line(line, column?)`
- `helix_get_hover(line, column, path?, allow_insert?)`
- `helix_get_definition(line, column, path?)`
- `helix_get_references(line, column, path?)`
- `helix_get_diagnostics(path?)`
- `helix_get_workspace_symbols(query)`
- `helix_format_document(path?)`
- `helix_run_command(name, args)`

Names use underscores rather than dots. MCP's specification allows `.` in tool names, but Claude Code's validator is stricter — `^[a-zA-Z0-9_-]+$` is the safe alphabet. Avoid dots to dodge any client-side rejection.

### 7.4 Discovery flow

When `serve` starts:
1. Read `CLAUDE_PROJECT_DIR` from env (Claude Code sets this for stdio MCP servers).
2. Glob `$CLAUDE_PROJECT_DIR/.helix/control-*.sock` and `$CLAUDE_PROJECT_DIR/.helix/control-*.sock.path`.
3. For pointer files (`*.sock.path`), read their contents to get the real socket path.
4. For each candidate path: attempt `connect()`. Stale Unix-domain sockets return `ECONNREFUSED` instantly when the listening process is gone, and `ENOENT` if the file vanished mid-glob. Both are filtered out without a timeout. Add a 200 ms timeout as defense-in-depth against an edge case where the file exists, the kernel queue still accepts, but the listening process hangs.
5. Among live sockets:
   - If exactly one: use it.
   - If multiple (two Helix instances in same directory — rare): use the one with the newest filesystem mtime.
6. If no live socket found: serve MCP, but every Tool call returns error "no Helix instance attached." Resources still return the snapshot file from `<workspace>/.helix/context.json` if present.

So if Helix isn't running at all, Claude still gets passive context via Resources reading the file. Tools just refuse cleanly. The MCP server never errors out at startup — it just operates in degraded mode.

**No global registry, no PID liveness checks against `/proc`, no cross-workspace scans.** Discovery is entirely scoped to the project directory the MCP server was invoked from.

**Lazy connection:** the MCP server opens the socket on the first Tool call, not at server startup. This avoids holding a socket if Claude only ever reads Resources.

### 7.5 Connection lifecycle

- **Lazy Helix connection:** the MCP server opens the Unix socket on the first MCP tool call, not at server startup. Avoids holding a socket if Claude only reads Resources from the snapshot file.
- **Reconnect on Helix restart:** on socket I/O error during a tool call, the MCP server retries discovery (re-globs `.helix/control-*.sock`) and reconnects on the next call. Single retry per call; failure surfaces as MCP error to Claude.
- **MCP server never panics on socket errors** — returns structured MCP errors instead.
- **Stdio MCP server crash semantics:** if the MCP server *itself* crashes mid-session, Claude Code **does not** automatically respawn it. Stdio servers stay dead until the Claude Code session restarts (verified against current docs). This is an upstream Claude Code limitation, not something we can fix. Implementation implication: avoid panics aggressively; treat the MCP server as needing to be defensively-correct against malformed input from either side.

### 7.6 Hook subcommand dedup logic

```
input: stdin JSON {session_id, cwd, transcript_path, hook_event_name, permission_mode, ...}
env (best-effort): CLAUDE_PROJECT_DIR

1. Read stdin JSON. `session_id` and `cwd` come from here. `CLAUDE_SESSION_ID` is **not** an env var (verified).
2. If `--reset-marker` flag set (called by `PostCompact` / `SessionStart compact`):
   - Remove `$XDG_RUNTIME_DIR/claude-helix/marker-${session_id}`
   - Exit 0
3. Locate snapshot:
   - Try `$CLAUDE_PROJECT_DIR/.helix/context.json` first
   - If `CLAUDE_PROJECT_DIR` unset, try `$cwd/.helix/context.json`, then walk up looking for `.helix/context.json`
   - If missing after all paths, exit 0 silently
4. Read snapshot. Check `last_update_source`:
   - If `"mcp_command"`: exit 0 (Claude already knows from the tool response that produced this state)
5. Compute snapshot mtime
6. Read marker file `$XDG_RUNTIME_DIR/claude-helix/marker-${session_id}`:
   - If marker mtime == snapshot mtime: exit 0 (already injected this version)
7. Print snapshot wrapped in `<helix-editor-context>` tags
8. Write current snapshot mtime to marker file
9. Exit 0
```

Marker storage:
- Use `$XDG_RUNTIME_DIR/claude-helix/` (mode 0700, per-user) on Linux
- Use `~/Library/Caches/claude-helix/` on macOS
- Use `~/.cache/claude-helix/` as fallback
- Never `/tmp/` (world-writable, security risk)

Session ID is read from the hook's stdin JSON (`session_id` field — verified against current Claude Code docs), not from an env var. `cwd` is also read from the same JSON to anchor file lookups when `CLAUDE_PROJECT_DIR` is unset. Marker filenames embed the session_id verbatim.

## 8. Claude Code integration

### 8.1 `.mcp.json` (project-scoped, committed to repo)

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

No `"type": "stdio"` field — stdio is implicit from the presence of `command`+`args` (verified against current Claude Code MCP docs). The `type` field appears only for `http` or `sse` transports.

### 8.2 `~/.claude/settings.json` hooks

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "helix-claude-mcp hook",
            "timeout": 5
          }
        ]
      }
    ],
    "PostCompact": [
      {
        "hooks": [
          {"type": "command", "command": "helix-claude-mcp hook --reset-marker"}
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "compact",
        "hooks": [
          {"type": "command", "command": "helix-claude-mcp hook --reset-marker"}
        ]
      }
    ]
  }
}
```

## 9. Shared schema crate (`helix-context-schema`)

Lives at `helix-context-schema/` in the workspace. Pure data types, no runtime dependencies beyond `serde` and `serde_json`. Both `helix-term` and `helix-claude-mcp` depend on it as a workspace path dep.

Public types:
- `ContextSnapshot` (the JSON file shape, including the optional `instance` hint block)
- `UpdateSource` enum (`FocusLost`, `McpCommand`, `Manual`)
- `Instance` (the optional `snapshot.instance` block shape — PID + socket-path hint)
- `ControlRequest` and `ControlResponse` enums (every JSON-RPC method)
- `JsonRpcError` codes

Schema changes happen here once and surface as compile errors on both sides.

## 9b. Operational failure modes

| Failure | Detection | Behavior |
|---|---|---|
| Helix crashes while MCP server is connected | Socket I/O error (`EPIPE`, `ECONNRESET`) | MCP server marks connection dead; next tool call retries discovery. Resources still serve from snapshot file. |
| MCP server crashes mid-session | stdio EOF on Claude Code's side | Claude Code does **not** auto-respawn stdio MCP. Server stays dead until session restart. Mitigation: aggressive defensive coding, no panics in normal paths. |
| Helix handler panics during a control request | `oneshot::Sender` dropped without sending | Socket client task receives `RecvError::Closed` on the receiver, surfaces as JSON-RPC `-32603 InternalError`. |
| LSP query hangs indefinitely | No reply | Socket task wraps the LSP future in `tokio::time::timeout(10s)`; on timeout returns `-32002 LspTimeout`. |
| Hook script exceeds 5s timeout | Claude Code kills the process | No injection happens for that prompt; conversation continues without context. User can hit retry. |
| Two clients connect to the control socket simultaneously | Both accept; both serve | Each connection is an independent `mpsc` producer to the editor event channel. Requests interleave but Editor mutations are serialized through the event loop. No data race; some commands may run out-of-submit-order. |
| Snapshot file is read mid-rewrite | jq parse error in shell hook | The atomic `tmp + rename` pattern means readers either see fully-old or fully-new content. On rare NFS edge cases, hook retries one parse before giving up. |
| Disk full when writing snapshot | `ENOSPC` in `write_context_file` | Log at warn level, return without surfacing to user. Next focus-loss retry. |
| EditorEvent channel full | (Currently unbounded `mpsc`; if bounded in future) | Drop the request with `-32603` to avoid blocking the socket task. |

The MCP server's defensive principle: **prefer returning a structured MCP error to crashing.** Claude Code's lack of stdio respawn means every panic is a session-ending event.

## 10. Risk register (from audit)

| Risk | Severity | Mitigation |
|---|---|---|
| Multi-instance discovery | Minor (was Blocker) | Project-local sockets eliminate cross-workspace ambiguity. Worktrees are separate workspaces by `find_workspace`. Same-directory dual-Helix resolved by newest-mtime tiebreak (§7.4). |
| Stale socket files | Minor (was Major) | MCP discovery filters via `connect()` (§7.4). Helix unlinks orphaned own-path on startup (§5.3). Optional background hygiene task (§5.3). |
| Editor mutation marshaling | Blocker | `EditorEvent::ControlRequest` + `oneshot::Sender` (§5.4) |
| Long LSP queries freezing editor | Major | Socket task awaits LSP futures, not main loop (§5.4) |
| Snapshot schema versioning | Minor | Bump to v2 + `min_supported_reader` (§4) |
| JSON-RPC version mismatch | Major | `initialize` handshake (§6.1) |
| Spurious Steel hooks | Major | Call `write_context_file` directly, not via event dispatch (§5.5) |
| Scratch-buffer writes to $HOME | Major | When `is_cwd_fallback=true`, skip the snapshot write entirely (§4) |
| Mid-typing LSP queries | Major | Mode-aware refusal with `allow_insert_mode` flag (§6.5) |
| Marker file in /tmp | Major | Use `$XDG_RUNTIME_DIR` per-user dir (§7.6) |
| Socket path length on macOS | Minor | Pointer-file fallback to runtime dir for absurd paths (§5.2) |
| `.helix/` directory permissions on multi-user systems | Minor | `chmod 0600` on socket immediately after bind (§5.2) |
| MCP server crash recovery | Minor | Socket reconnect on next tool call (§7.5) |

## 11. Phased implementation plan

### Phase 1 — Fix existing bugs and bump schema (small)

- Fix `is_cwd_fallback` bug in current `context_logger.rs` (line 24 destructures as `_is_cwd_fallback`, value discarded). When `true`, skip the snapshot write entirely instead of silently polluting `$HOME/.helix/context.json`. **Note this is a behavior change** for users launching `hx` outside any workspace marker — they will stop getting a snapshot file. Document in release notes.
- **Extend `write_context_file` signature to `write_context_file(editor: &Editor, source: UpdateSource)`** — current shipped signature takes only `&Editor` (line 18). All Phase 1 callers pass `UpdateSource::FocusLost`. Listed explicitly because it's a Phase 1 deliverable, not a Phase 2 footnote.
- Extract types into `helix-context-schema` workspace crate. Keep this crate's deps minimal (serde + serde_json) so the heavier `rmcp` dependency that arrives in Phase 4 doesn't transitively touch `helix-term`'s compile graph.
- Bump snapshot to v2 with `last_update_source` (always populated) and an optional `instance` block. In Phase 1 the `instance` block is omitted entirely; Phase 2 fills it in once the listener exists.
- Update existing shell hook (`~/.claude/hooks/helix-context.sh`) to check for `mcp_command` source via a `grep -q '"last_update_source": "mcp_command"'` and skip injection if matched. Current hook does not check this field today; adding the check is forward-compatible with Phase 2's introduction of `mcp_command` sources.
- For the non-fallback user case, the only behavior change is the new `last_update_source` field in the JSON. Existing readers tolerate unknown keys (`jq` defaults to `null` for missing fields and `cat`-based readers are insensitive to extra keys).

### Phase 2 — Add control socket (medium)

- Add `[editor.control-socket]` config (default `enabled = false` — feature is opt-in)
- Implement socket path resolution per §5.2 (project-local with macOS pointer-file fallback)
- Implement `UnixListener` bind, `chmod 0600`, and event-loop integration (`EditorEvent::ControlRequest`)
- Implement startup orphan check for own PID-suffixed path
- Implement graceful shutdown unlink in `Application::close`
- Implement read methods (no LSP yet): `initialize`, `current-state`, `get-buffer-text`, `get-open-buffers`
- Implement basic write methods: `open-file`, `goto-line`
- Wire `write_context_file(Source::McpCommand)` directly (not via `helix_event::dispatch`) after mutating methods
- Populate the `instance` block in the snapshot once the listener is up

### Phase 3 — LSP-backed methods (medium)

- `get-hover-at`, `get-definition-at`, `get-references-at`, `get-diagnostics`, `get-workspace-symbols`
- Mode-aware refusal in insert mode (Mode::Insert → error `-32003` unless `allow_insert_mode: true` was passed)
- **LSP request capture pattern (concrete plan):** The main loop must extract the LSP request parameters before yielding. Pattern:
  1. Handler runs on main task with `&mut Editor`. Looks up `language_servers_with_feature(Feature)`, picks the right server, calls `client.text_document_hover(...)` (or equivalent) which returns an owned `impl Future<Output = ...> + Send`.
  2. Move the future into the `ControlResponse::Pending(future)` variant (or send via a side `mpsc<Future>` channel to the socket task).
  3. Socket task awaits the future, sends result back via the originating `oneshot::Sender`.
  4. The captured future owns its parameters (document URL, position, text-encoding tag), so it's `Send + 'static` without referencing `Editor` after the await point. Verify by spot-checking `helix-lsp::Client::text_document_hover` return type during Phase 3 implementation.

### Phase 4 — Build `helix-claude-mcp` binary (medium)

- New workspace member crate
- `rmcp`-based stdio MCP server (`serve` subcommand)
- Discovery: glob `$CLAUDE_PROJECT_DIR/.helix/control-*.sock` (§7.4)
- Lazy socket connection on first Tool call
- Tools and Resources implementations forwarding to JSON-RPC

### Phase 5 — Replace shell hook with Rust `hook` subcommand (small)

- Implement `hook` and `hook --reset-marker` in same binary
- Update `~/.claude/settings.json` to point at it
- Update `.mcp.json` template
- Delete shell hook

### Phase 6 — Polish (small)

- `format-document`, `run-command` socket methods
- Notifications/resources/list_changed if Claude exhibits staleness
- Telemetry / debug logs (use `log::info!`/`log::debug!` with target `helix_term::context_logger` and `helix_term::control_socket`; existing `helix-term` logger config picks these up)
- **`:write-context` typable command:** user-facing command that calls `write_context_file(editor, UpdateSource::Manual)`. Lets the user force a snapshot refresh (e.g., before switching panes if focus-loss didn't fire, or for debugging). This is what `UpdateSource::Manual` exists for; the variant otherwise has no producer.
- `helix-claude-mcp doctor` subcommand for self-diagnosis (per Open Question 2)

Each phase is independently shippable. Phase 1 alone is valuable (bug fix + schema crate); phases 2-3 give Approach 2 minus the MCP bridge; phase 4 completes the user-facing feature.

## 12. Open questions

1. **Should `.mcp.json` be committed to user's project repos** or kept in their dotfiles? Trade-off: committed = team gets it for free; dotfiles = user controls. Recommend committed once the binary is stable.
2. **Should we offer a `helix-claude-mcp doctor` subcommand** that checks: binary on PATH, snapshot file present and parseable, sockets in `<workspace>/.helix/` connectable, JSON-RPC `initialize` succeeds? Useful for support.
3. **License for the new binary** — same as Helix (MPL-2.0) or different? Probably same to keep it simple.
4. **Whether to publish to crates.io** — not yet; keep in workspace until API stabilizes.
5. **Notifications/resources/list_changed** — defer until we observe staleness in practice. May not be needed.

## 13. Things we explicitly are NOT building

- Native MCP implementation inside Helix
- Steel-based MCP server (Steel runtime lacks the primitives)
- Kitty/Tmux remote-control fallback
- HTTP/SSE MCP transport
- Helix-initiated AI interactions (separate problem, separate spec)
- Generic editor-control framework usable by other editors (this is Helix-specific)

## Appendix A — Sources

- MCP spec 2025-11-25: https://modelcontextprotocol.io/specification/2025-11-25
- Official Rust SDK: https://github.com/modelcontextprotocol/rust-sdk
- Claude Code MCP docs: https://code.claude.com/docs/en/mcp.md
- Claude Code hooks docs: https://code.claude.com/docs/en/hooks-guide.md
- Helix architecture: `docs/architecture.md`, `helix-term/src/application.rs`
- Existing `context_logger`: `helix-term/src/context_logger.rs`
- Steel plugin system events: `helix-term/src/events.rs`
