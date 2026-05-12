# Phase 2c — Write Methods + Snapshot Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close out Phase 2 with state-mutating methods over the control socket. Add `open-file` and `goto-line` methods that change editor state, snapshot rewriting after every mutation (tagged `mcp_command` so the hook can skip injection), the bounded-channel migration that the Phase 2b reviewer flagged as a write-methods prerequisite, and `snapshot.instance` block population for discovery.

**Architecture:** Builds directly on Phase 2b's marshaling pipeline. Write methods use the same `handle_control_request` path Phase 2b created; the difference is they mutate `self.editor` and immediately call `crate::context_logger::write_context_file(&self.editor, UpdateSource::McpCommand)` before returning the response. The snapshot rewrite uses the direct function call (not `helix_event::dispatch(TerminalFocusLost)`) per spec §5.5 — this avoids firing user Steel hooks spuriously on every MCP command. The unbounded mpsc is replaced with a bounded one (capacity 64) and `handle_connection` switches to async `.send().await` for proper backpressure.

**Tech Stack:** Same as Phases 2a/2b — Tokio, serde, serde_json. No new external deps.

**Spec:** `docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md` §5.5 (snapshot rewrite on mutation), §6.3 (write methods). The Phase 2b final review (commit `2ee154d3e`) flagged the bounded channel as a Phase 2c prereq.

---

## Project context for the implementer

You are working in a Helix editor fork at `/Users/angm/helix` on branch `nightly`. Phases 1, 2a, and 2b are complete (tip: `2ee154d3e`, 34 commits ahead of remote). The control socket runs, accepts JSON-RPC, handles `initialize`/`current-state`/`get-open-buffers`/`get-buffer-text`. All 36 tests pass.

What Phase 2c adds:

- `ControlRequest` variants: `OpenFile { path }`, `GotoLine { line, column?, path? }`. Matching `ControlResponse::Ok {}` minimal-payload variants.
- `Application` field tracking the live socket session (pid, socket_path, started_at) — enough info to populate the `Instance` block.
- A bounded mpsc channel (`mpsc::channel(64)`) replacing the unbounded one in Phase 2b.
- `handle_connection` switches to async `.send().await` on the channel for proper backpressure.
- `handle_control_request` arms for `OpenFile` and `GotoLine` that mutate editor state then call `write_context_file(..., UpdateSource::McpCommand)`.
- `build_snapshot` accepts an optional `Instance` parameter; the MCP-command path passes `Some(instance)`, the focus-loss path passes `None`.
- `initialize` capabilities advertise `["open-file", "goto-line"]` under `write_methods`.

What Phase 2c does NOT do:

- `run-typable-command` — generic command exec is deferred to a follow-up (constructing `Args` and handling `PromptEvent` is messy enough to warrant its own plan).
- LSP-backed methods (Phase 3).
- The external `helix-claude-mcp` bridge binary (Phase 4).
- The Rust `hook` subcommand (Phase 5).

## File structure

**Modify:**

- `helix-context-schema/src/protocol.rs` — add `OpenFile` and `GotoLine` to `ControlRequest`; add corresponding `ControlResponse::Ok {}` variant (single minimal response, since both writes just signal success).
- `helix-context-schema/tests/protocol_roundtrip.rs` — round-trip tests for the new variants.
- `helix-term/src/control_socket/dispatch.rs` — route `OpenFile` and `GotoLine` to `None` (event-loop dispatch); advertise them in capabilities.
- `helix-term/src/control_socket/server.rs` — `handle_connection` uses `.send(event).await` instead of sync `.send()`.
- `helix-term/src/application.rs`:
  - Replace `mpsc::unbounded_channel` with `mpsc::channel(64)`.
  - Add `ControlSocketSession` struct (pid + socket_path + started_at) and store it on `Application`.
  - `handle_control_request` arms for `OpenFile`/`GotoLine` that mutate then write snapshot.
  - Pass `Instance` into `build_snapshot` calls where available.
- `helix-term/src/context_logger.rs` — `build_snapshot` and `write_context_file` accept an `Option<Instance>` parameter. The existing focus-loss call site (in `ui/editor.rs`) passes `None`.
- `helix-term/src/ui/editor.rs` — update the existing `write_context_file` call site to pass the new `Option<Instance>` argument as `None`.

**No new files.**

---

## Task 1: Migrate to bounded `mpsc::channel(64)` with backpressure

**Files:**
- Modify: `helix-term/src/application.rs`
- Modify: `helix-term/src/control_socket/server.rs`

This task is a foundation for write methods. Reviewers flagged unbounded as a real risk under high-throughput write traffic. We do it first so the rest of the phase exercises the bounded path.

- [ ] **Step 1: Update the channel type in `Application`**

Edit `helix-term/src/application.rs`. Find the field:

```rust
    control_request_rx:
        Option<tokio::sync::mpsc::UnboundedReceiver<helix_view::editor::EditorEvent>>,
```

Change to:

```rust
    control_request_rx:
        Option<tokio::sync::mpsc::Receiver<helix_view::editor::EditorEvent>>,
```

- [ ] **Step 2: Update `Application::new` to construct a bounded channel**

Find:

```rust
        let (control_socket_binding, control_request_rx) = if editor.config().control_socket.enabled {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<helix_view::editor::EditorEvent>();
            // ...
```

Change `unbounded_channel::<helix_view::editor::EditorEvent>()` to `channel::<helix_view::editor::EditorEvent>(64)`.

- [ ] **Step 3: Update `start_control_socket` signature**

Find the signature taking `tokio::sync::mpsc::UnboundedSender<...>`. Change to `tokio::sync::mpsc::Sender<...>`. Body is unchanged.

- [ ] **Step 4: Update `recv_control_request` signature**

Find the helper:

```rust
async fn recv_control_request(
    rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<helix_view::editor::EditorEvent>>,
) -> Option<helix_view::editor::EditorEvent> {
```

Change `UnboundedReceiver` to `Receiver`. Body is unchanged (`.recv()` works the same on both).

- [ ] **Step 5: Update `run_accept_loop` and `handle_connection` signatures**

Edit `helix-term/src/control_socket/server.rs`. Find:

```rust
use tokio::sync::mpsc::UnboundedSender;
```

Change to:

```rust
use tokio::sync::mpsc::Sender;
```

In `run_accept_loop`'s signature, change `control_tx: UnboundedSender<EditorEvent>` to `control_tx: Sender<EditorEvent>`. Same for `handle_connection`.

- [ ] **Step 6: Switch the send call to async `.send().await`**

Find inside `handle_connection`:

```rust
                if control_tx.send(event).is_err() {
                    log::warn!("control-socket: control_tx send failed; editor likely shutting down");
                    break;
                }
```

Change to:

```rust
                if control_tx.send(event).await.is_err() {
                    log::warn!("control-socket: control_tx send failed; editor likely shutting down");
                    break;
                }
```

Note the `.await`. Bounded `Sender::send` is async — it waits if the channel is full. If the receiver is dropped (editor shutting down), it returns `Err`.

- [ ] **Step 7: Update the e2e test in `server.rs`**

Find the test that creates the channel:

```rust
let (control_tx, _control_rx) = tokio::sync::mpsc::unbounded_channel::<helix_view::editor::EditorEvent>();
```

Change to:

```rust
let (control_tx, _control_rx) = tokio::sync::mpsc::channel::<helix_view::editor::EditorEvent>(64);
```

- [ ] **Step 8: Verify build and tests**

Run: `cargo check --workspace`
Expected: Clean.

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 23 + 13 = 36 tests pass.

- [ ] **Step 9: Smoke test the bounded path still works**

```bash
mkdir -p /tmp/p2c-t1 && cd /tmp/p2c-t1 && git init -q && echo "fn main(){}" > main.rs
cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 1.5
SOCK=$(ls /tmp/p2c-t1/.helix/control-*.sock | head -1)
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"current-state\",\"params\":{}}\n')
print(json.dumps(json.loads(s.recv(8192).decode()), indent=2)[:300])
"
pkill -P $HX_PID; kill $HX_PID; sleep 0.5
rm -rf /tmp/p2c-t1
```

Expected: same response as Phase 2b's smoke test — `method=current-state`, `result.active.path=main.rs`. The bounded channel is transparent to clients.

- [ ] **Step 10: Commit**

```bash
git add helix-term/src/application.rs helix-term/src/control_socket/server.rs
git commit -m "$(cat <<'EOF'
refactor(control-socket): bounded mpsc::channel(64) with async send

Replaces the unbounded sender/receiver with a bounded channel of capacity
64. handle_connection switches to async .send().await for proper
backpressure — the connection task waits if the editor is processing
slowly, rather than queueing unboundedly into the editor's mailbox.

Capacity 64 is generous for interactive use; an MCP bridge running 1-2
clients will never fill it. If a misbehaving client spams writes, the
socket task pauses at .await rather than OOMing the editor.

Same wire behavior — verified end-to-end against current-state.

Refs: Phase 2b final review (commit 2ee154d3e) flagged unbounded as a
write-methods prerequisite.
EOF
)"
```

---

## Task 2: Add `OpenFile` and `GotoLine` to the protocol schema

**Files:**
- Modify: `helix-context-schema/src/protocol.rs`
- Modify: `helix-context-schema/tests/protocol_roundtrip.rs`
- Modify: `helix-term/src/control_socket/dispatch.rs`

- [ ] **Step 1: Write failing tests**

Append to `helix-context-schema/tests/protocol_roundtrip.rs`:

```rust
#[test]
fn open_file_request_serializes_with_path() {
    let req = ControlRequest::OpenFile { path: "src/main.rs".into() };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "open-file");
    assert_eq!(j["params"]["path"], "src/main.rs");
}

#[test]
fn open_file_request_round_trips() {
    let json = serde_json::json!({
        "method": "open-file",
        "params": { "path": "src/lib.rs" }
    });
    let req: ControlRequest = serde_json::from_value(json).unwrap();
    let ControlRequest::OpenFile { path } = req else {
        panic!("wrong variant");
    };
    assert_eq!(path, "src/lib.rs");
}

#[test]
fn goto_line_request_with_only_line() {
    let req = ControlRequest::GotoLine { line: 42, column: None, path: None };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "goto-line");
    assert_eq!(j["params"]["line"], 42);
    assert!(j["params"].get("column").is_none() || j["params"]["column"].is_null());
    assert!(j["params"].get("path").is_none() || j["params"]["path"].is_null());
}

#[test]
fn goto_line_request_with_all_fields() {
    let req = ControlRequest::GotoLine {
        line: 10,
        column: Some(5),
        path: Some("src/main.rs".into()),
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["params"]["line"], 10);
    assert_eq!(j["params"]["column"], 5);
    assert_eq!(j["params"]["path"], "src/main.rs");
}

#[test]
fn ok_response_serializes_with_empty_result() {
    let resp = ControlResponse::Ok {};
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "ok");
    assert_eq!(j["result"], serde_json::json!({}));
}
```

- [ ] **Step 2: Run, confirm failure**

Run: `cargo test -p helix-context-schema`
Expected: 5 new tests fail — variants don't exist.

- [ ] **Step 3: Add the variants**

Edit `helix-context-schema/src/protocol.rs`. Find the `ControlRequest` enum. Add two new variants after `GetBufferText`:

```rust
    OpenFile {
        path: String,
    },
    GotoLine {
        line: usize,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        column: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
    },
```

Find `ControlResponse` and add the `Ok` variant after `GetBufferText`:

```rust
    /// Generic success response for state-mutating methods (open-file,
    /// goto-line). Carries no payload — the client used the tool, the tool
    /// worked, that's all there is to say.
    Ok {},
```

- [ ] **Step 4: Stub in `dispatch.rs`**

Edit `helix-term/src/control_socket/dispatch.rs`. Find `try_dispatch_inline`:

```rust
        ControlRequest::CurrentState {}
        | ControlRequest::GetOpenBuffers {}
        | ControlRequest::GetBufferText { .. } => None,
```

Extend to include the new variants:

```rust
        ControlRequest::CurrentState {}
        | ControlRequest::GetOpenBuffers {}
        | ControlRequest::GetBufferText { .. }
        | ControlRequest::OpenFile { .. }
        | ControlRequest::GotoLine { .. } => None,
```

- [ ] **Step 5: Stub in `application.rs::handle_control_request`**

Edit `helix-term/src/application.rs`. Find `handle_control_request`. Add arms for the new variants (return `MethodNotFound` stubs — Task 4 and 5 implement the real handlers):

```rust
            ControlRequest::OpenFile { .. } => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "open-file handler not yet implemented".into(),
                    data: None,
                })
            }
            ControlRequest::GotoLine { .. } => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "goto-line handler not yet implemented".into(),
                    data: None,
                })
            }
```

- [ ] **Step 6: Verify build + tests**

Run: `cargo check --workspace`
Expected: Clean.

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 28 (23+5) + 13 = 41 tests pass.

- [ ] **Step 7: Commit**

```bash
git add helix-context-schema/src/protocol.rs helix-context-schema/tests/protocol_roundtrip.rs helix-term/src/control_socket/dispatch.rs helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(context-schema): add OpenFile and GotoLine protocol variants

ControlRequest::OpenFile { path } — opens a buffer at the given path.
ControlRequest::GotoLine { line, column?, path? } — moves the cursor;
defaults to active buffer and column 1 when optional fields are absent.
ControlResponse::Ok {} — generic success for state-mutating methods.

dispatch.rs routes both new variants to None (event-loop handling).
application.rs returns MethodNotFound stubs — real handlers in Task 4/5.

Five new serde round-trip tests cover wire format and optional-field
omission.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.3)
EOF
)"
```

---

## Task 3: Plumb `Option<Instance>` through `build_snapshot` and `write_context_file`

**Files:**
- Modify: `helix-term/src/context_logger.rs`
- Modify: `helix-term/src/ui/editor.rs`

This task is a small pre-refactor that lets future write-method handlers populate `snapshot.instance` when they call `write_context_file`. The focus-loss path keeps passing `None` (it doesn't have access to `Application`'s socket session).

- [ ] **Step 1: Update `write_context_file` signature**

Edit `helix-term/src/context_logger.rs`. Find:

```rust
pub fn write_context_file(editor: &Editor, source: UpdateSource) -> std::io::Result<()> {
```

Change to:

```rust
pub fn write_context_file(
    editor: &Editor,
    source: UpdateSource,
    instance: Option<helix_context_schema::Instance>,
) -> std::io::Result<()> {
```

Inside the body, find the `let snapshot = build_snapshot(...)` call and add the new argument:

```rust
    let snapshot = build_snapshot(editor, &workspace, &cfg, source, instance);
```

- [ ] **Step 2: Update `build_snapshot` signature**

In the same file, find:

```rust
pub(crate) fn build_snapshot(
    editor: &Editor,
    workspace: &Path,
    cfg: &ContextLoggerConfig,
    source: UpdateSource,
) -> ContextSnapshot {
```

Change to:

```rust
pub(crate) fn build_snapshot(
    editor: &Editor,
    workspace: &Path,
    cfg: &ContextLoggerConfig,
    source: UpdateSource,
    instance: Option<helix_context_schema::Instance>,
) -> ContextSnapshot {
```

Find the `ContextSnapshot { ... instance: None, ... }` construction at the bottom of `build_snapshot`. Replace `instance: None,` with `instance,`.

- [ ] **Step 3: Update the focus-loss caller**

Edit `helix-term/src/ui/editor.rs`. Find the call to `write_context_file`:

```rust
        if let Err(e) = crate::context_logger::write_context_file(
            context.editor,
            UpdateSource::FocusLost,
        ) {
```

Add the third argument:

```rust
        if let Err(e) = crate::context_logger::write_context_file(
            context.editor,
            UpdateSource::FocusLost,
            None,
        ) {
```

- [ ] **Step 4: Update the `CurrentState` arm in `handle_control_request`**

This arm uses `build_snapshot` directly. Edit `helix-term/src/application.rs`. Find:

```rust
                    let snap = crate::context_logger::build_snapshot(
                        &self.editor,
                        &workspace,
                        &cfg,
                        helix_context_schema::UpdateSource::Manual,
                    );
```

Add the fifth argument:

```rust
                    let snap = crate::context_logger::build_snapshot(
                        &self.editor,
                        &workspace,
                        &cfg,
                        helix_context_schema::UpdateSource::Manual,
                        None,  // Phase 2c Task 6 will plumb the real Instance here
                    );
```

- [ ] **Step 5: Verify**

Run: `cargo check --workspace`
Expected: Clean.

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 41 tests pass.

- [ ] **Step 6: Commit**

```bash
git add helix-term/src/context_logger.rs helix-term/src/ui/editor.rs helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
refactor(context-logger): thread Option<Instance> through build_snapshot

write_context_file and build_snapshot now accept an Option<Instance>
that gets stamped into ContextSnapshot.instance when Some. Callers that
don't know the running socket session (focus-loss, current-state) pass
None. Task 6 will plumb a real Instance from Application's socket
session into the MCP-command path.

No behavior change yet — every existing caller passes None.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§4, §5.5)
EOF
)"
```

---

## Task 4: Implement `open-file` handler

**Files:**
- Modify: `helix-term/src/application.rs`

Helix's `Editor::open` takes a path and an `Action` (the focus disposition). The simplest action is `Action::Replace` — replace the focused view with the new buffer. Alternative: `Action::Load` to load but not focus, or `Action::VerticalSplit`/`HorizontalSplit` to split.

For Phase 2c we use `Action::Replace` (focus the opened file). Future plans can expose the action choice via a protocol parameter.

- [ ] **Step 1: Verify `Editor::open` and `Action` are reachable**

Run: `grep -n "pub fn open\|enum Action\|pub enum Action" /Users/angm/helix/helix-view/src/editor.rs | head -10`

Confirm there's a `pub fn open` on `Editor` and a public `Action` enum. The `Action` enum should have variants including `Replace`.

If your codebase has the API at slightly different names, adjust the handler accordingly — the goal is "open a path and focus the resulting buffer."

- [ ] **Step 2: Replace the `OpenFile` stub**

Edit `helix-term/src/application.rs`. Find the `ControlRequest::OpenFile { .. }` stub in `handle_control_request`. Replace with:

```rust
            ControlRequest::OpenFile { path } => {
                let (workspace, is_cwd_fallback) = helix_loader::find_workspace();
                let resolved_path: std::path::PathBuf = {
                    let p = std::path::Path::new(&path);
                    if p.is_absolute() {
                        p.to_path_buf()
                    } else if is_cwd_fallback {
                        // No workspace marker — interpret relative path against CWD
                        // since there's no better anchor.
                        std::env::current_dir().unwrap_or_default().join(p)
                    } else {
                        workspace.join(p)
                    }
                };

                match self.editor.open(&resolved_path, helix_view::editor::Action::Replace) {
                    Ok(_) => {
                        // Snapshot rewrite per spec §5.5 — direct call, not via
                        // helix_event::dispatch, to avoid firing Steel hooks.
                        if let Err(e) = crate::context_logger::write_context_file(
                            &self.editor,
                            helix_context_schema::UpdateSource::McpCommand,
                            None, // Task 6 fills in Some(Instance)
                        ) {
                            log::warn!("control-socket: snapshot rewrite failed after open-file: {}", e);
                        }
                        Ok(ControlResponse::Ok {})
                    }
                    Err(e) => Err(JsonRpcError {
                        code: JsonRpcErrorCode::InternalError,
                        message: format!("failed to open {}: {}", resolved_path.display(), e),
                        data: None,
                    }),
                }
            }
```

- [ ] **Step 3: Verify build**

Run: `cargo check --workspace`
Expected: Clean. If `Editor::open`'s exact signature differs (e.g. returns `Result<DocumentId, _>` vs `Result<(), _>`), adjust the `match` arm. The handler ignores the success payload (`Ok(_)`) so any `Ok` shape works.

- [ ] **Step 4: Run tests**

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 41 pass.

- [ ] **Step 5: Build release**

Run: `cargo build --release -p helix-term --bin hx`
Expected: Succeeds.

- [ ] **Step 6: Smoke-test**

```bash
mkdir -p /tmp/p2c-of && cd /tmp/p2c-of && git init -q
echo "fn main() {}" > main.rs
echo "// other" > other.rs
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 1.5
SOCK=$(ls /tmp/p2c-of/.helix/control-*.sock | head -1)

# Open other.rs via the socket
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"open-file\",\"params\":{\"path\":\"other.rs\"}}\n')
print('open-file response:', s.recv(4096).decode())
"

# Verify it's now focused via current-state
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"current-state\",\"params\":{}}\n')
r = json.loads(s.recv(8192).decode())
print('active path after open:', r['result']['active']['path'])
"

# Confirm the snapshot file was written with mcp_command source
cat /tmp/p2c-of/.helix/context.json 2>/dev/null | python3 -c "
import sys, json
d = json.load(sys.stdin)
print('last_update_source:', d.get('last_update_source'))
print('active.path:', d.get('active', {}).get('path'))
"

pkill -P $HX_PID; kill $HX_PID; sleep 0.5
rm -rf /tmp/p2c-of
```

Expected output:
- `open-file response: {"method":"ok","result":{}}\n`
- `active path after open: other.rs`
- `last_update_source: mcp_command`
- `active.path: other.rs`

If the snapshot file isn't created — that's expected when `context-logger` config has `enabled = false`. To exercise the full path enable both `[editor.context-logger]` and `[editor.control-socket]` in your config before the test.

- [ ] **Step 7: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement open-file method

Resolves the path (absolute or workspace-relative; falls back to CWD when
launched outside a workspace marker) and calls Editor::open with
Action::Replace.

After successful open, rewrites the context snapshot via direct
write_context_file call (NOT via helix_event::dispatch) so user Steel
hooks registered for terminal-focus-lost don't fire spuriously on every
MCP command. Source is tagged "mcp_command" so the Claude hook script
can skip injection (Claude already knows it caused this change).

Returns ControlResponse::Ok on success, InternalError with the open()
error message on failure.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§5.5, §6.3)
EOF
)"
```

---

## Task 5: Implement `goto-line` handler

**Files:**
- Modify: `helix-term/src/application.rs`

Goto-line moves the cursor to a 1-indexed line (and optionally column) in either the active buffer or a specified one. Helix's API for cursor moves is via `Selection::point(char_idx)` on the document. The Phase 2b `resolve_buffer` helper handles path resolution.

- [ ] **Step 1: Replace the `GotoLine` stub**

Edit `helix-term/src/application.rs`. Find the `ControlRequest::GotoLine { .. }` stub. Replace with:

```rust
            ControlRequest::GotoLine { line, column, path } => {
                let (workspace, _) = helix_loader::find_workspace();
                // resolve_buffer takes &Editor and gives us a &Document. We need
                // to ALSO open the buffer if a path was given but isn't currently
                // open — but for Phase 2c we keep it strict: error if not open,
                // user is expected to call open-file first.
                let doc_id = match resolve_buffer(&self.editor, &workspace, path.as_deref()) {
                    Ok(doc) => doc.id(),
                    Err(e) => {
                        let _ = reply.send(Err(e));
                        return;
                    }
                };

                // Switch the focused view to this document if it's not already.
                let current_doc_id = self
                    .editor
                    .documents
                    .get(&self.editor.tree.get(self.editor.tree.focus).doc)
                    .map(|d| d.id());
                if Some(doc_id) != current_doc_id {
                    self.editor.switch(doc_id, helix_view::editor::Action::Replace);
                }

                // Compute the char index of the target line/column.
                let view = self.editor.tree.get(self.editor.tree.focus);
                let view_id = view.id;
                let doc = match self.editor.documents.get_mut(&doc_id) {
                    Some(d) => d,
                    None => {
                        let _ = reply.send(Err(JsonRpcError {
                            code: JsonRpcErrorCode::NoActiveDocument,
                            message: "document gone after switch".into(),
                            data: None,
                        }));
                        return;
                    }
                };
                let text = doc.text();
                // 1-indexed → 0-indexed; clamp to last line.
                let target_line = line.saturating_sub(1).min(text.len_lines().saturating_sub(1));
                let line_start_char = text.line_to_char(target_line);
                let target_col = column.unwrap_or(1).saturating_sub(1);
                // Cap at the line's length to avoid off-the-end columns.
                let line_end_char = if target_line + 1 < text.len_lines() {
                    text.line_to_char(target_line + 1).saturating_sub(1)
                } else {
                    text.len_chars()
                };
                let char_idx = (line_start_char + target_col).min(line_end_char);

                let selection = helix_core::Selection::point(char_idx);
                doc.set_selection(view_id, selection);

                if let Err(e) = crate::context_logger::write_context_file(
                    &self.editor,
                    helix_context_schema::UpdateSource::McpCommand,
                    None, // Task 6
                ) {
                    log::warn!("control-socket: snapshot rewrite failed after goto-line: {}", e);
                }

                let _ = reply.send(Ok(ControlResponse::Ok {}));
                return;
            }
```

Note: this arm has multiple early returns via `reply.send(...)` then `return;` because the borrow checker can't easily hold `&mut self.editor` across the existing match's value-returning arm. The other arms in `handle_control_request` use a single `Ok(_)` value at the end. This arm restructures as "do work, send reply via the oneshot, return early." Both patterns are fine.

If the existing `handle_control_request` ends with a single `let _ = reply.send(resp);` line that handles all arms via the `resp` value — adjust this arm to early-return BEFORE that line. The other arms remain unchanged.

- [ ] **Step 2: Verify build**

Run: `cargo check --workspace`
Expected: Clean. Likely lint about `view_id` shadowing or borrow patterns — adjust by renaming locals as needed.

- [ ] **Step 3: Run tests**

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 41 pass.

- [ ] **Step 4: Smoke test**

```bash
mkdir -p /tmp/p2c-gl && cd /tmp/p2c-gl && git init -q
cat > main.rs <<'EOF'
fn main() {
    println!("line 2");
    println!("line 3");
    println!("line 4");
    println!("line 5");
}
EOF
cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 1.5
SOCK=$(ls /tmp/p2c-gl/.helix/control-*.sock | head -1)

# Jump to line 4, column 9
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"goto-line\",\"params\":{\"line\":4,\"column\":9}}\n')
print('goto-line response:', s.recv(4096).decode())
"

# Verify the cursor is at line 4 column 9
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"current-state\",\"params\":{}}\n')
r = json.loads(s.recv(8192).decode())
print('cursor after goto-line:', r['result']['active']['cursors'][0])
"

pkill -P $HX_PID; kill $HX_PID; sleep 0.5
rm -rf /tmp/p2c-gl
```

Expected:
- `goto-line response: {"method":"ok","result":{}}\n`
- `cursor after goto-line: {'primary': True, 'line': 4, 'column': 9}` (approximately; exact column may vary if the line is shorter than 9 chars, clamping behavior kicks in)

- [ ] **Step 5: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement goto-line method

Moves the cursor to a 1-indexed line (column defaults to 1). When path
is given, the buffer must already be open — use open-file first.
Optionally switches the focused view to that buffer.

Clamps the target line to [0, line_count-1] and the target column to
the line's actual length, so out-of-range requests don't error or panic.

After cursor move, rewrites the snapshot via direct write_context_file
(not helix_event::dispatch — avoids firing Steel hooks). Tagged
mcp_command source so the Claude hook can skip redundant injection.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.3)
EOF
)"
```

---

## Task 6: Populate `snapshot.instance` from `Application`'s socket session

**Files:**
- Modify: `helix-term/src/application.rs`

So far every `write_context_file`/`build_snapshot` call passes `instance: None`. This task populates `Some(Instance { pid, socket_path, started_at })` for the MCP-command path, so external tools reading the snapshot can find the live socket without scanning.

- [ ] **Step 1: Add a `ControlSocketSession` struct**

Edit `helix-term/src/application.rs`. Near `start_control_socket` (or at module scope), add:

```rust
/// Captures what the live control-socket session needs to advertise itself
/// in snapshot.instance. Built once at Application::new when the socket is
/// enabled, then cloned/borrowed into snapshot writes via the
/// MCP-command path.
#[derive(Debug, Clone)]
struct ControlSocketSession {
    resolved: crate::control_socket::path::Resolved,
    pid: u32,
    started_at: String,
}

impl ControlSocketSession {
    fn to_instance(&self) -> helix_context_schema::Instance {
        helix_context_schema::Instance {
            pid: self.pid,
            socket_path: match self.resolved.pointer_target.as_deref() {
                Some(real) => real.to_string_lossy().into_owned(),
                None => self.resolved.primary.to_string_lossy().into_owned(),
            },
            started_at: self.started_at.clone(),
        }
    }
}
```

- [ ] **Step 2: Replace `control_socket_binding` field type with `ControlSocketSession`**

Find:

```rust
    control_socket_binding: Option<crate::control_socket::path::Resolved>,
```

Change to:

```rust
    control_socket_binding: Option<ControlSocketSession>,
```

- [ ] **Step 3: Update `start_control_socket` to return `ControlSocketSession`**

Find:

```rust
fn start_control_socket(
    editor: &helix_view::Editor,
    control_tx: tokio::sync::mpsc::Sender<helix_view::editor::EditorEvent>,
) -> std::io::Result<crate::control_socket::path::Resolved> {
```

Change the return type to `Result<ControlSocketSession>`. At the end of the function, instead of `Ok(resolved_for_cleanup)`, build and return a session:

```rust
    Ok(ControlSocketSession {
        resolved: resolved_for_cleanup,
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
    })
```

- [ ] **Step 4: Update `Application::close` to unlink via session**

Find:

```rust
        if let Some(resolved) = self.control_socket_binding.take() {
            if let Err(e) = crate::control_socket::lifecycle::unlink(&resolved) {
                log::warn!("control-socket: failed to unlink at shutdown: {}", e);
            }
        }
```

Change `resolved` to `session` and pass `&session.resolved`:

```rust
        if let Some(session) = self.control_socket_binding.take() {
            if let Err(e) = crate::control_socket::lifecycle::unlink(&session.resolved) {
                log::warn!("control-socket: failed to unlink at shutdown: {}", e);
            }
        }
```

- [ ] **Step 5: Pass `Some(instance)` from MCP-command write methods**

In `handle_control_request`, the `OpenFile` and `GotoLine` arms currently call `write_context_file(..., None)`. Change to use the session's instance:

```rust
                        let instance = self.control_socket_binding.as_ref().map(|s| s.to_instance());
                        if let Err(e) = crate::context_logger::write_context_file(
                            &self.editor,
                            helix_context_schema::UpdateSource::McpCommand,
                            instance,
                        ) {
                            log::warn!("control-socket: snapshot rewrite failed: {}", e);
                        }
```

Repeat for both `OpenFile` and `GotoLine` arms.

Also update the `CurrentState` arm's `build_snapshot` call to pass `Some(instance)` when the session exists:

```rust
                    let instance = self.control_socket_binding.as_ref().map(|s| s.to_instance());
                    let snap = crate::context_logger::build_snapshot(
                        &self.editor,
                        &workspace,
                        &cfg,
                        helix_context_schema::UpdateSource::Manual,
                        instance,
                    );
```

- [ ] **Step 6: Verify**

Run: `cargo check --workspace`
Expected: Clean.

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 41 pass.

- [ ] **Step 7: Smoke test**

```bash
mkdir -p /tmp/p2c-inst && cd /tmp/p2c-inst && git init -q
echo "fn main() {}" > main.rs
echo "// other" > other.rs

# Need context-logger enabled to see the snapshot file
cat > /tmp/p2c-inst/.helix/config.toml <<'EOF'
[editor.context-logger]
enabled = true

[editor.control-socket]
enabled = true
EOF

cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 1.5
SOCK=$(ls /tmp/p2c-inst/.helix/control-*.sock | head -1)

# Trigger an MCP-command write
python3 -c "
import socket
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"open-file\",\"params\":{\"path\":\"other.rs\"}}\n')
print(s.recv(4096).decode())
"
sleep 0.5

cat /tmp/p2c-inst/.helix/context.json | python3 -c "
import sys, json
d = json.load(sys.stdin)
print('last_update_source:', d.get('last_update_source'))
print('instance:', json.dumps(d.get('instance'), indent=2))
"

pkill -P $HX_PID; kill $HX_PID; sleep 0.5
rm -rf /tmp/p2c-inst
```

Expected:
- `last_update_source: mcp_command`
- `instance: { "pid": <hx_pid>, "socket_path": "/tmp/p2c-inst/.helix/control-<pid>.sock", "started_at": "<rfc3339 timestamp>" }`

Note: per-workspace `.helix/config.toml` is only loaded if the workspace is already trusted in Helix's trust DB. If your smoke test machine doesn't trust `/tmp/p2c-inst`, either trust it manually first or run the test against your global `~/.config/helix/config.toml` (with context-logger enabled).

- [ ] **Step 8: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): populate snapshot.instance from socket session

Adds ControlSocketSession (pid + resolved path + started_at) — captured
once at Application::new when the socket comes up — and threads it into
build_snapshot via the MCP-command and Manual paths. The focus-loss
path keeps passing None.

External tools reading the snapshot can now discover the live socket
via .instance.socket_path without scanning the .helix/ directory.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§4, §5)
EOF
)"
```

---

## Task 7: Advertise write methods + final end-to-end smoke

**Files:**
- Modify: `helix-term/src/control_socket/dispatch.rs`

- [ ] **Step 1: Update the capabilities test**

Edit `helix-term/src/control_socket/dispatch.rs`. Find the existing test `initialize_advertises_all_phase_2b_read_methods` (renamed in Phase 2b Task 8). Add a sibling test or extend it to assert write_methods now lists `["open-file", "goto-line"]`:

```rust
    #[test]
    fn initialize_advertises_phase_2c_write_methods() {
        let req = ControlRequest::Initialize {
            protocol_version: "1.0".into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let resp = try_dispatch_inline(&req).unwrap().unwrap();
        let ControlResponse::Initialize { capabilities, .. } = resp else {
            panic!("expected Initialize response");
        };
        let writes = &capabilities.write_methods;
        assert!(writes.contains(&"open-file".to_string()), "missing open-file");
        assert!(writes.contains(&"goto-line".to_string()), "missing goto-line");
    }
```

You can also tighten the existing read-methods test to assert `write_methods.len() == 2`.

- [ ] **Step 2: Run, confirm new test fails**

Run: `cargo test -p helix-term control_socket::dispatch`
Expected: New test fails (write_methods is empty).

- [ ] **Step 3: Update `handle_initialize`**

Find:

```rust
        capabilities: ServerCapabilities {
            read_methods: vec![
                "initialize".into(),
                "current-state".into(),
                "get-open-buffers".into(),
                "get-buffer-text".into(),
            ],
            write_methods: vec![],
        },
```

Change `write_methods` to:

```rust
            write_methods: vec![
                "open-file".into(),
                "goto-line".into(),
            ],
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p helix-term control_socket`
Expected: 14 pass (13 existing + 1 new).

- [ ] **Step 5: Final end-to-end smoke test (all six methods)**

```bash
mkdir -p /tmp/p2c-final && cd /tmp/p2c-final && git init -q
cat > main.rs <<'EOF'
fn main() {
    let x = 1;
    let y = 2;
    let z = 3;
}
EOF
echo "// other" > other.rs

cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs other.rs &
HX_PID=$!
sleep 2
SOCK=$(ls /tmp/p2c-final/.helix/control-*.sock | head -1)

python3 <<PYEOF
import socket, json
def call(method, params=None):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect('$SOCK')
    req = {'method': method, 'params': params or {}}
    s.sendall((json.dumps(req) + '\n').encode())
    return json.loads(s.recv(16384).decode())

print('=== initialize ===')
r = call('initialize', {'protocol_version': '1.0', 'client_info': {'name': 'final', 'version': '0.1'}})
print(json.dumps(r['result']['capabilities'], indent=2))
print()

print('=== current-state (before any changes) ===')
r = call('current-state')
print('active path:', r['result']['active']['path'])
print('cursor:', r['result']['active']['cursors'][0])
print()

print('=== open-file other.rs ===')
print(call('open-file', {'path': 'other.rs'}))
print()

print('=== current-state (after open) ===')
r = call('current-state')
print('active path:', r['result']['active']['path'])
print()

print('=== goto-line in main.rs line 3 col 5 ===')
print(call('goto-line', {'line': 3, 'column': 5, 'path': 'main.rs'}))
print()

print('=== current-state (after goto-line) ===')
r = call('current-state')
print('active path:', r['result']['active']['path'])
print('cursor:', r['result']['active']['cursors'][0])
PYEOF

pkill -P $HX_PID; kill $HX_PID; sleep 0.5
rm -rf /tmp/p2c-final
```

Expected outline (specifics may vary):
- `initialize.result.capabilities.read_methods` has 4 entries; `write_methods` has 2 (`open-file`, `goto-line`)
- After `open-file other.rs`, current-state shows `active.path = "other.rs"`
- After `goto-line {line:3, col:5, path:main.rs}`, current-state shows `active.path = "main.rs"` and `cursors[0] = {line: 3, column: 5}` (or clamped if line 3 is shorter than 5 chars)

- [ ] **Step 6: Commit**

```bash
git add helix-term/src/control_socket/dispatch.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): advertise Phase 2c write methods in capabilities

handle_initialize.capabilities.write_methods now lists "open-file" and
"goto-line". One new test asserts this; the existing Phase 2b test
remains tight on read_methods.

Final end-to-end smoke covers all six methods: initialize, current-state,
get-open-buffers, get-buffer-text (all from Phase 2b) plus open-file
and goto-line. Editor mutations via MCP correctly trigger snapshot
rewrites tagged mcp_command and populate the instance block.

Phase 2 of the spec is now complete. Phase 3 adds LSP-backed methods.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.1, §6.3)
EOF
)"
```

---

## Self-review checklist

- [ ] `cargo check --workspace` clean
- [ ] `cargo test -p helix-context-schema` 28 pass (23 prior + 5 new)
- [ ] `cargo test -p helix-term control_socket` 14 pass (13 prior + 1 new from Task 7)
- [ ] Final smoke test in Task 7 passes — all six methods return expected results
- [ ] `git log --oneline -10` shows the 7 Phase 2c commits cleanly

## What's NOT in Phase 2c

- **`run-typable-command`** — generic command exec needs `Args` construction and `PromptEvent` handling. Defer to a Phase 2d if needed.
- **LSP-backed methods** (Phase 3): `get-hover-at`, `get-definition-at`, `get-references-at`, `get-diagnostics`, `get-workspace-symbols`, `get-workspace-symbols`.
- **`format-document`** — could be Phase 2d.
- **`helix-claude-mcp` external binary** (Phase 4).
- **Steel hook bypass verification** — manually verifying that a user-registered Steel `terminal-focus-lost` hook does NOT fire on MCP commands is worth doing once after Phase 2c. Not part of any task.

## Open questions

1. **Pre-trust requirement for per-workspace config.** Helix's per-workspace `.helix/config.toml` is only loaded if the workspace is trusted. Smoke tests that depend on per-workspace config need to either trust the workspace or use the global config. This is unchanged from Phase 2a/2b.

2. **`Editor::open` API stability.** This plan assumes `editor.open(&path, Action::Replace)` returns `Result<_>`. If the actual signature is different in this fork's Helix branch, the implementer adjusts on the fly — the goal is "open a file and focus it."

3. **Action choice fixed to `Action::Replace`.** Future plans can expose `action: "replace" | "load" | "split-horizontal" | "split-vertical"` as a protocol parameter. For Phase 2c the simplest user model is "open this file; it replaces the focus."

4. **Per-method response variants vs a single `Ok`.** Choosing the single `ControlResponse::Ok {}` keeps wire size small and avoids variant proliferation for trivially-empty payloads. If future write methods need to return data (e.g. `open-file` returning the resulting buffer's DocumentId), they can add their own variant — `Ok` is for the "did it, nothing to report" case.
