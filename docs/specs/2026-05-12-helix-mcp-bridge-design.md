# Helix ↔ Claude Code Bridge — Design Spec

**Status:** Phases 1–6 shipped on `nightly` (2026-05-13)
**Date:** 2026-05-12 (updated 2026-05-13)
**Author:** Angjelko
**Supersedes:** Extends `helix-term/src/context_logger.rs` (shipped 2026-05-11)

This document is kept aligned with the implementation. Wire formats, response shapes, and field names in §6 reflect what the code actually does — earlier drafts have been corrected as drift was discovered.

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
│  │             │                            │ helix-mcp   │    │
│  │             │                            │   hook             │    │
│  │             │◀───reads .helix/context.json (passive prefetch)─│    │
│  │             │                            │                    │    │
│  │             │◀──MCP (stdio) Tools+Resources──┐                │    │
│  └─────────────┘                                │                │    │
└─────────────────────────────────────────────────┼────────────────┘    │
                                                  │                     │
                                ┌─────────────────▼──────────────────┐  │
                                │ helix-mcp serve             │  │
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
            │       (types used by both Helix and helix-mcp)│
            └──────────────────────────────────────────────────────┘
```

### Components

- **`helix-context-schema`** (new shared crate): the JSON snapshot types + JSON-RPC method definitions. Depended on by both `helix-term` and `helix-mcp`.
- **Helix-side changes** (in `helix-term`):
  - Control socket: Unix-domain server at `<workspace>/.helix/control-<pid>.sock` (with macOS path-length fallback — see §5.2)
  - Editor-event channel: `EditorEvent::ControlRequest` variant carrying request + `oneshot` reply sender
  - Snapshot extension: add `last_update_source` field (always set); optional `instance` block (PID + socket_path hint, informational only)
  - Workspace fallback fix: when `find_workspace` returns `is_cwd_fallback=true`, skip writing the snapshot (don't pollute `$HOME/.helix/`)
  - No global instance registry. No startup directory scan. Discovery is project-local.
- **`helix-mcp` binary** (new repo or workspace member):
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
3. **Path-length fallback:** Unix `sun_path` is 104 bytes on macOS and 108 on Linux. The implementation uses a conservative 104-byte cap on both platforms (cheaper than `cfg` branching, and the extra 4 bytes on Linux rarely changes whether a path fits). If the resolved path from (2) exceeds the cap, fall back to `$XDG_RUNTIME_DIR/helix/control-<pid>-<workspace_hash>.sock` (or `$TMPDIR/...` on macOS) and write a small pointer file at `<workspace>/.helix/control-<pid>.sock.path` containing the real path. MCP-server discovery checks for `*.sock.path` pointers first.

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

In `helix-term/src/application.rs::event_loop_until_idle`, add a new arm to `tokio::select!`. The arm sits after `input_stream` and the job callback channels, before `editor.wait_event()` — with `biased`, this gives local user keystrokes and pending jobs priority over MCP traffic, which matches the user-trust ordering (typing > Claude). The handler is invoked via a tiny `recv_control_request` helper so `Option<Receiver>` (the channel is `None` when the control socket is disabled) can use `pending()` to never fire:

```rust
Some(event) = recv_control_request(&mut self.control_request_rx) => {
    self.handle_control_request(event);
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

JSON-RPC-inspired framing over a single-newline-delimited stream per connection. **Not strictly JSON-RPC 2.0** — we drop the `jsonrpc: "2.0"` envelope and the `id` field, because each connection is single-flight (one request in flight, the response immediately follows). The `method` and `params` keys on the wire, plus the matching `method`/`result` shape on responses, retain JSON-RPC's idiom. Each connection is one client (typically `helix-mcp serve`).

Field names on the wire are `snake_case` throughout, matching every other field on the protocol (including the JSON snapshot schema).

### 6.1 Lifecycle

- `initialize` — handshake. Client sends `{protocol_version: "1.0", client_info: {name, version}}`; Helix returns `{protocol_version, helix_version, server_info, capabilities: {read_methods, write_methods}}`. Reject mismatched majors.

(There is no `shutdown` method — per-connection close is implicit via socket close; no graceful cross-connection shutdown protocol exists.)

### 6.2 Read methods (no state mutation)

| Method | Params | Returns |
|---|---|---|
| `current-state` | `{}` | `{active: {...same shape as snapshot.active}, mode: string}` |
| `get-buffer-text` | `{path?: string, range?: {start_line, end_line}}` | `{text, language?, line_count}` |
| `get-diagnostics` | `{path?: string}` (defaults to current file) | `{diagnostics: [LspDiagnostic]}` |
| `get-hover-at` | `{line, column, path?, allow_insert_mode?}` | `{hover: LspHover \| null}` (LSP-backed) |
| `get-definition-at` | `{line, column, path?, allow_insert_mode?}` | `{locations: [LspLocation]}` (LSP-backed) |
| `get-references-at` | `{line, column, path?, allow_insert_mode?, include_declaration?}` | `{locations: [LspLocation]}` (LSP-backed; `include_declaration` defaults `true`) |
| `get-workspace-symbols` | `{query: string}` | `{symbols: [LspSymbolInfo]}` (LSP-backed) |
| `get-open-buffers` | `{}` | `{buffers: [OpenBuffer]}` |

Shared LSP types (defined in `helix-context-schema`):
- `LspPosition`: `{line: u32, character: u32}` — 0-indexed, LSP semantics
- `LspRange`: `{start: LspPosition, end: LspPosition}`
- `LspLocation`: `{path: string, path_abs: string, range: LspRange}`
- `LspHover`: `{contents: string, range?: LspRange}` (flattened from LSP's MarkupContent variants)
- `LspDiagnostic`: `{range: LspRange, severity?: "error"|"warning"|"information"|"hint", code?, source?, message: string}`
- `LspSymbolInfo`: `{name: string, kind: string, location: LspLocation, container_name?: string}`

### 6.3 Write methods (mutate state, trigger snapshot rewrite)

| Method | Params | Returns |
|---|---|---|
| `open-file` | `{path, line?, column?}` | `Ok {}` (when `line` is given, the cursor jumps and the view recenters — useful as a "show me where you're about to edit" call) |
| `goto-line` | `{line, column?, path?}` | `Ok {}` (view recenters on the target line so the user sees surrounding context) |
| `select-range` | `{start_line, start_column, end_line, end_column, path?}` | `Ok {}` (1-indexed inclusive; `start` is anchor, `end` is head; view recenters on head) |
| `run-command` | `{name, args: []}` | `{message?: string}` (last status text from the editor) |
| `format-document` | `{path?}` | `{applied: bool}` (true once the format was kicked off; the LSP edits arrive asynchronously) |

`open-file` and `goto-line` use the shared `Ok {}` variant — success is signaled by being on the Ok branch of the response. There is no `{ok: true}` payload field.

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

## 7. MCP server binary (`helix-mcp`)

### 7.1 Crate organization

A new workspace member at `helix-mcp/` containing:

```
helix-mcp/
├── Cargo.toml
├── src/
│   ├── main.rs        # CLI: subcommand dispatch
│   ├── serve.rs       # MCP stdio server (rmcp ServerHandler)
│   ├── hook.rs        # UserPromptSubmit hook
│   ├── discovery.rs   # Find Helix instance for current workspace
│   ├── rpc_client.rs  # JSON-RPC client over Unix socket
│   ├── resources.rs   # MCP Resources (snapshot file reader)
│   └── tools.rs       # MCP Tools enum + arg schemas
```

Dependencies:
- `rmcp` (official Rust MCP SDK, `modelcontextprotocol/rust-sdk`)
- `tokio`, `serde`, `serde_json`, `anyhow`, `clap`
- `helix-context-schema` (shared crate, workspace path dep)

### 7.2 Subcommands

```
helix-mcp serve              # stdio MCP server
helix-mcp hook [--reset-marker]
                                    # UserPromptSubmit / PostCompact / SessionStart hook
```

The `hook` subcommand reads JSON from stdin (Claude Code's hook payload), uses `CLAUDE_PROJECT_DIR` env var to find the right snapshot, applies dedup, prints to stdout (or skips silently).

### 7.2b Server instructions (vendor-neutral agent onboarding)

The MCP `initialize` response populates the optional `instructions` field with a tight operating manual: what the resources and tools are, the navigate-before-edit workflow, insert-mode safety rules, and how to degrade when Helix isn't running. Any compliant MCP client (Claude Code, Codex CLI, Cursor, Cline, Continue, Zed, …) feeds these instructions to the LLM as part of its system context, so every coding agent learns the same playbook automatically — no per-agent rules files needed.

The instructions live in `helix-mcp/src/serve.rs` as `SERVER_INSTRUCTIONS`. Treat them as part of the public contract: changing tool behavior requires updating the instructions in the same commit. The integration test `initialize_handshake_succeeds` asserts the field is present and non-empty, so a silent regression that drops the instructions block fails CI.

### 7.3 MCP Resources vs Tools mapping

Per convention (Resources = static state, Tools = LLM-initiated actions/queries):

**Resources** (Claude reads via `resources/read`):
- `helix://state/current` — current cursor, selection, file, mode
- `helix://state/buffers` — list of open buffers
- `helix://state/snapshot` — same as the .helix/context.json content

**Tools** (Claude calls via `tools/call`):
- `helix_open_file(path, line?, column?)`
- `helix_goto_line(line, column?, path?)`
- `helix_select(start_line, start_column, end_line, end_column, path?)`
- `helix_get_hover(line, column, path?, allow_insert_mode?)`
- `helix_get_definition(line, column, path?, allow_insert_mode?)`
- `helix_get_references(line, column, path?, allow_insert_mode?, include_declaration?)`
- `helix_get_diagnostics(path?)`
- `helix_get_workspace_symbols(query)`
- `helix_format_document(path?)`
- `helix_run_command(name, args)`

`helix_open_file` accepts an optional `line` (and `column`) so a single tool call opens the file and centers the view on the target — typically used by Claude immediately before making edits, so the user sees where the change is about to land. Clients that need to navigate inside an already-open buffer use `helix_goto_line` instead.

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

- **Per-call connection:** the MCP server opens a fresh Unix socket per tool call (each connection is single-flight). Earlier drafts mentioned "open on first call and reuse"; the actual implementation is simpler — connect, send, receive, close. A future connection-pool refactor is possible if profiling shows the connect cost matters.
- **Reconnect on Helix restart:** on socket I/O error, the next tool call re-globs `.helix/control-*.sock` from scratch. There is no in-call retry; a failed call surfaces as an MCP error to Claude and the next call will attempt fresh discovery.
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
5. Compute snapshot mtime. If `now - mtime > 24h`: exit 0 (stale — a day-old snapshot is rarely useful and quietly dropping it avoids confusing Claude with context from a long-closed session).
6. Read marker file `$XDG_RUNTIME_DIR/claude-helix/marker-${session_id}`:
   - If marker mtime == snapshot mtime: exit 0 (already injected this version)
7. Print snapshot wrapped in `<helix-editor-context>` tags
8. Write current snapshot mtime to marker file
9. Exit 0

The `session_id` is sanitized before being used as a filename: any character outside `[A-Za-z0-9_-]` is replaced with `_`. Today's session ids are UUIDs (already safe), but the sanitization defends against unexpected formats and prevents path traversal via a malformed session id.
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
      "command": "helix-mcp",
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
            "command": "helix-mcp hook",
            "timeout": 5
          }
        ]
      }
    ],
    "PostCompact": [
      {
        "hooks": [
          {"type": "command", "command": "helix-mcp hook --reset-marker"}
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "compact",
        "hooks": [
          {"type": "command", "command": "helix-mcp hook --reset-marker"}
        ]
      }
    ]
  }
}
```

## 9. Shared schema crate (`helix-context-schema`)

Lives at `helix-context-schema/` in the workspace. Pure data types, no runtime dependencies beyond `serde` and `serde_json`. Both `helix-term` and `helix-mcp` depend on it as a workspace path dep.

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
| Editor event channel full | Bounded `mpsc::channel(64)` between socket task and editor | `send().await` applies backpressure to the socket task; if the channel stays full, accepting new connections still works but per-connection request submission blocks. In practice the editor drains 64 requests in milliseconds, so this is theoretical. |

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
| Checked-in malicious `.sock.path` pointer redirecting MCP traffic | Minor | Bridge requires the pointer target to live under one of the runtime prefixes Helix would write to (`$XDG_RUNTIME_DIR/helix/`, `$TMPDIR/helix/` on macOS, `$XDG_CACHE_HOME/helix/`). Out-of-prefix targets are dropped with a warning. Pointer files capped at 4 KiB. |
| Frame reader OOM via no-newline stream | Minor | `FrameReader` enforces `MAX_FRAME_BYTES = 1 MiB` *before* extending its buffer, via an explicit fill_buf/consume loop — no `read_line`/`take` adapter, so unbounded buffering is impossible. |
| Marker directory created world-readable | Minor | Hook writes the marker dir with `DirBuilder::mode(0o700)` on unix so the 0o755 umask window doesn't expose it; belt-and-suspenders `set_permissions(0o700)` still runs for pre-existing dirs. |
| Snapshot text breaks `<helix-editor-context>` fence | Minor | `emit_wrapped_snapshot` scans the body for the literal closing tag and skips emission (log warn, exit 0) when found. The hook is best-effort so a skipped emission costs the user nothing — the prompt proceeds without an inlined snapshot. Pure helper `snapshot_body_is_safe_to_wrap` is unit-tested. |
| Bridge → Helix RPC hangs on a stuck editor | Minor | `send_request_with_timeout` wraps every tool RPC in a 30 s deadline (`DEFAULT_RPC_TIMEOUT`). Cache invalidates on transport errors so a Helix restart auto-recovers on the next call. |
| Protocol drift between bridge and Helix | Minor | Bridge sends `initialize` on first tool call and caches the outcome per process; major-version mismatch returns `HandshakeOutcome::Incompatible` and dispatch_tool surfaces a friendly upgrade message instead of a parse failure. |

## 10b. Known limitations / accepted residual risk

These are real-but-bounded concerns surfaced in the post-Phase-6 audit that we are deliberately not closing in this round. Listed here so a future implementer can find them and decide whether the calculus has changed.

- **`helix_run_command` allows `:write` (and similar recoverable mutations).** Force-quits and shell-execs are denied (`is_destructive_typable_command` in `application.rs`); `:write`, `:reload`, `:format`, `:theme`, `:set`, etc. remain reachable. `:write` to an unintended path could overwrite a file, but the result is recoverable via the user's VCS or undo — unlike the denied commands. Users who explicitly want unrestricted access can set `HELIX_CONTROL_SOCKET_ALLOW_DESTRUCTIVE=1` before starting Helix.
- **`helix_open_file` accepts absolute paths.** No canonicalize-and-prefix-check. Opening `/etc/shadow` is permitted at the protocol layer; the resulting buffer's contents flow back into the snapshot. Same trust assumption as above. A future hardening pass could add a workspace-confinement option.
- **No overall timeout on bridge→Helix RPC.** Discovery has a 200 ms connect timeout, but once connected, `write_all`/`read_line` run indefinitely. If Helix's event loop hangs (e.g., a Steel hook blocking the main thread), the MCP tool call hangs with it. A future hardening pass could wrap `send_request` in `tokio::time::timeout(30s)` and map elapsed to an MCP error. Today Claude Code's own per-tool timeout limits the blast radius.
- **`FormatDocument` reports `applied: true` before the LSP edits actually arrive.** This is documented in the tool description ("Returns applied: true when the format was kicked off; the actual edits arrive asynchronously"). Clients waiting on the actual diff need to re-read the buffer or poll. A future enhancement could surface a request id and a follow-up `notifications/format_completed`.

These are intentionally listed here rather than in §10's risk register so the register stays focused on risks we have actually mitigated. The items above are mitigated only by trust assumptions or downstream timeouts.

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

### Phase 4 — Build `helix-mcp` binary (medium)

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

Shipped:
- `format-document`, `run-command` socket methods (and matching `helix_format_document` / `helix_run_command` MCP tools).
- Telemetry / debug logs (use `log::info!`/`log::debug!` with target `helix_term::context_logger` and `helix_term::control_socket`; existing `helix-term` logger config picks these up).
- **`:write-context` typable command:** user-facing command that calls `write_context_file(editor, UpdateSource::Manual)`. Lets the user force a snapshot refresh (e.g., before switching panes if focus-loss didn't fire, or for debugging). This is what `UpdateSource::Manual` exists for; the variant otherwise has no producer. The command reports a different status when `context-logger` is disabled or Helix was launched outside a workspace marker (so users don't get a misleading "written" message when nothing was actually written).

Deferred (not shipped, not currently required — see §10b for the rationale):
- `notifications/resources/list_changed` if Claude exhibits staleness.
- `helix-mcp doctor` subcommand for self-diagnosis (per Open Question 2).
- A `--verbose` flag on the hook with telemetry breadcrumbs.

The deferred items can land as a Phase 6b polish round once the rest of the bridge has bedded in.

Each phase is independently shippable. Phase 1 alone is valuable (bug fix + schema crate); phases 2-3 give Approach 2 minus the MCP bridge; phase 4 completes the user-facing feature.

## 12. Open questions

1. **Should `.mcp.json` be committed to user's project repos** or kept in their dotfiles? Trade-off: committed = team gets it for free; dotfiles = user controls. Recommend committed once the binary is stable.
2. **Should we offer a `helix-mcp doctor` subcommand** that checks: binary on PATH, snapshot file present and parseable, sockets in `<workspace>/.helix/` connectable, JSON-RPC `initialize` succeeds? Useful for support.
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
