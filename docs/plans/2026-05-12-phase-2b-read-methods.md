# Phase 2b — Read Methods Over the Control Socket — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the control socket actually useful for reading editor state. Add three read methods — `current-state`, `get-open-buffers`, `get-buffer-text` — and the marshaling plumbing that lets per-connection tokio tasks reach into the main event loop where `&mut Editor` is available.

**Architecture:** A `tokio::sync::mpsc::UnboundedSender<EditorEvent>` flows from `Application::new` down through `start_control_socket` → `run_accept_loop` → `handle_connection`. When a request can't be served inline (i.e. needs editor state), `handle_connection` constructs a `oneshot::Sender`, packages it with the request as `EditorEvent::ControlRequest`, sends it to the channel, and awaits the response. `Application::event_loop_until_idle` gets a new `tokio::select!` arm that receives from the channel's `Receiver` and dispatches to `Application::handle_control_request(request, reply)` — a sync (non-async) method that has `&mut self.editor` available and can build typed responses immediately.

**Tech Stack:** Same as Phase 2a — Tokio, serde + serde_json. No new external deps.

**Spec:** `docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md` §6.2 (read methods) and §5.4 (event-loop marshaling pattern). Phase 2a final review (commit `4f1fa1fa9`) flagged the seams that Phase 2b would extend.

---

## Project context for the implementer

You are working in a Helix editor fork at `/Users/angm/helix` on branch `nightly`. Phase 2a is complete and merged (commits `e54186a43..be765e231`, tip `be765e231`). The control socket binds on startup, accepts connections, completes `initialize` handshakes, and unlinks on exit. All 30 Phase 2a tests (17 schema + 13 control_socket) pass.

What Phase 2a left for Phase 2b:

- `EditorEvent::ControlRequest { request, reply }` variant exists but is never fired.
- A stub arm in `Application::handle_editor_event` (around line 796 of `application.rs`) `drop(reply)`s with a `TODO(control-socket)` comment. Since `Editor::wait_event` has no channel that can produce this variant, the arm is unreachable today.
- `server::handle_connection` returns `MethodNotFound` for any non-`initialize` request (line ~54).
- `dispatch::handle_initialize` advertises `read_methods: vec!["initialize"]` — Phase 2b extends this.

What Phase 2b adds:

- A `mpsc::UnboundedSender<EditorEvent>` plumbed from `Application::new` to per-connection tokio tasks.
- A new `tokio::select!` arm in `event_loop_until_idle` that receives `ControlRequest` events from the channel.
- `Application::handle_control_request` — synchronous method dispatching against `&mut self` (so editor state and types are reachable). Sends responses via the request's `oneshot::Sender`.
- Three new `ControlRequest`/`ControlResponse` variants: `CurrentState`, `GetOpenBuffers`, `GetBufferText`.
- Updated `initialize` capabilities advertising all four read methods.

What Phase 2b does NOT do: editor mutation (Phase 2c), LSP queries (Phase 3), `instance` block population in the snapshot (Phase 2c), MCP bridge binary (Phase 4).

## File structure

**Modify:**

- `helix-context-schema/src/protocol.rs` — add `CurrentState`, `GetOpenBuffers`, `GetBufferText` variants to both `ControlRequest` and `ControlResponse`; add `LineRange` helper type.
- `helix-context-schema/src/lib.rs` — re-export `LineRange`.
- `helix-context-schema/tests/protocol_roundtrip.rs` — serde round-trip tests for new variants.
- `helix-term/src/control_socket/dispatch.rs` — extend advertised capabilities; `try_dispatch_inline` continues returning `None` for the new variants (they need editor state).
- `helix-term/src/control_socket/server.rs` — `run_accept_loop` and `handle_connection` take an `UnboundedSender<EditorEvent>`; non-inline requests get forwarded via a `oneshot::Sender`-paired `EditorEvent::ControlRequest`.
- `helix-term/src/application.rs`:
  - `Application` gains a `control_request_rx: Option<UnboundedReceiver<EditorEvent>>` field.
  - `Application::new` creates the channel when control-socket is enabled, passes the sender into `start_control_socket`.
  - `start_control_socket` signature gains the sender parameter, passes it to `run_accept_loop`.
  - `event_loop_until_idle` gets a new `tokio::select!` arm receiving from `control_request_rx`.
  - New `handle_control_request(request, reply)` method handles the editor-state methods.
  - Stub arm in `handle_editor_event` is removed (the variant is no longer fed through `wait_event`).

**No new files** — Phase 2b extends what Phase 2a put in place.

## Type design (locked in advance for cross-task consistency)

### New `ControlRequest` variants

```rust
CurrentState {},                                  // no params
GetOpenBuffers {},                                // no params
GetBufferText {
    path: Option<String>,                         // workspace-relative or absolute; defaults to active buffer
    range: Option<LineRange>,                     // defaults to whole buffer
},
```

### New `ControlResponse` variants

```rust
CurrentState {
    active: helix_context_schema::Active,         // already exists in the schema crate
    mode: String,                                 // "normal" | "insert" | "select"
},
GetOpenBuffers {
    buffers: Vec<helix_context_schema::OpenBuffer>,
},
GetBufferText {
    text: String,
    language: Option<String>,
    line_count: usize,
},
```

### New helper type

```rust
/// A 1-indexed, inclusive line range. Matches the indexing used throughout
/// the snapshot schema.
pub struct LineRange {
    pub start_line: usize,
    pub end_line: usize,
}
```

### Wire format examples

Request:
```json
{"method":"current-state","params":{}}
{"method":"get-open-buffers","params":{}}
{"method":"get-buffer-text","params":{"path":"src/main.rs","range":{"start_line":10,"end_line":20}}}
```

Response (success):
```json
{"method":"current-state","result":{"active":{...},"mode":"normal"}}
{"method":"get-open-buffers","result":{"buffers":[...]}}
{"method":"get-buffer-text","result":{"text":"...","language":"rust","line_count":42}}
```

Response (error — e.g. no active document, path outside workspace):
```json
{"code":-32004,"message":"no active document"}
```

---

## Task 1: Add `CurrentState`, `GetOpenBuffers`, `GetBufferText` to the protocol schema

**Files:**
- Modify: `helix-context-schema/src/protocol.rs`
- Modify: `helix-context-schema/src/lib.rs`
- Modify: `helix-context-schema/tests/protocol_roundtrip.rs`

- [ ] **Step 1: Write the failing tests**

Append to `helix-context-schema/tests/protocol_roundtrip.rs`:

```rust
use helix_context_schema::{Active, Cursor, LineRange, OpenBuffer};

#[test]
fn current_state_request_serializes_with_empty_params() {
    let req = ControlRequest::CurrentState {};
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "current-state");
    assert_eq!(j["params"], serde_json::json!({}));
}

#[test]
fn current_state_response_round_trips() {
    let resp = ControlResponse::CurrentState {
        active: Active {
            path: Some("src/main.rs".into()),
            path_abs: Some("/repo/src/main.rs".into()),
            language: Some("rust".into()),
            modified: false,
            line_count: 100,
            cursors: vec![Cursor { primary: true, line: 1, column: 1 }],
            selections: vec![],
            text: None,
        },
        mode: "normal".into(),
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "current-state");
    assert_eq!(j["result"]["mode"], "normal");
    assert_eq!(j["result"]["active"]["language"], "rust");

    let back: ControlResponse = serde_json::from_value(j).unwrap();
    let ControlResponse::CurrentState { mode, active } = back else {
        panic!("wrong variant");
    };
    assert_eq!(mode, "normal");
    assert_eq!(active.language.as_deref(), Some("rust"));
}

#[test]
fn get_open_buffers_request_and_response_round_trip() {
    let req = ControlRequest::GetOpenBuffers {};
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-open-buffers");

    let resp = ControlResponse::GetOpenBuffers {
        buffers: vec![OpenBuffer {
            path: Some("src/lib.rs".into()),
            language: Some("rust".into()),
            modified: false,
        }],
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "get-open-buffers");
    assert_eq!(j["result"]["buffers"][0]["path"], "src/lib.rs");
}

#[test]
fn get_buffer_text_request_with_path_and_range() {
    let req = ControlRequest::GetBufferText {
        path: Some("src/main.rs".into()),
        range: Some(LineRange { start_line: 10, end_line: 20 }),
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "get-buffer-text");
    assert_eq!(j["params"]["path"], "src/main.rs");
    assert_eq!(j["params"]["range"]["start_line"], 10);
    assert_eq!(j["params"]["range"]["end_line"], 20);

    let back: ControlRequest = serde_json::from_value(j).unwrap();
    let ControlRequest::GetBufferText { path, range } = back else {
        panic!("wrong variant");
    };
    assert_eq!(path.as_deref(), Some("src/main.rs"));
    let r = range.expect("range expected");
    assert_eq!(r.start_line, 10);
    assert_eq!(r.end_line, 20);
}

#[test]
fn get_buffer_text_request_with_no_path_omits_field() {
    let req = ControlRequest::GetBufferText { path: None, range: None };
    let j = serde_json::to_value(&req).unwrap();
    assert!(j["params"].get("path").is_none() || j["params"]["path"].is_null());
    assert!(j["params"].get("range").is_none() || j["params"]["range"].is_null());
}

#[test]
fn get_buffer_text_response_round_trips() {
    let resp = ControlResponse::GetBufferText {
        text: "fn main() {}".into(),
        language: Some("rust".into()),
        line_count: 1,
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["result"]["text"], "fn main() {}");
    assert_eq!(j["result"]["line_count"], 1);
}
```

- [ ] **Step 2: Run failing tests**

Run: `cargo test -p helix-context-schema`
Expected: 6 new tests fail — types/variants don't exist yet.

- [ ] **Step 3: Add `LineRange`**

Edit `helix-context-schema/src/protocol.rs`. After the `ServerCapabilities` struct definition, before the `ControlRequest` enum, add:

```rust
/// A 1-indexed, inclusive line range. Matches the indexing used throughout
/// the snapshot schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub start_line: usize,
    pub end_line: usize,
}
```

- [ ] **Step 4: Add the new `ControlRequest` variants**

Find the existing `ControlRequest` enum. Extend it from:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "kebab-case")]
pub enum ControlRequest {
    Initialize {
        protocol_version: String,
        client_info: ClientInfo,
    },
}
```

to:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "kebab-case")]
pub enum ControlRequest {
    Initialize {
        protocol_version: String,
        client_info: ClientInfo,
    },
    CurrentState {},
    GetOpenBuffers {},
    GetBufferText {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        range: Option<LineRange>,
    },
}
```

- [ ] **Step 5: Add the new `ControlResponse` variants**

Find the existing `ControlResponse` enum and extend it. The new file should look like:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "kebab-case")]
pub enum ControlResponse {
    Initialize {
        protocol_version: String,
        helix_version: String,
        server_info: ServerInfo,
        capabilities: ServerCapabilities,
    },
    CurrentState {
        active: crate::types::Active,
        mode: String,
    },
    GetOpenBuffers {
        buffers: Vec<crate::types::OpenBuffer>,
    },
    GetBufferText {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        language: Option<String>,
        line_count: usize,
    },
}
```

Note: `crate::types::Active` and `crate::types::OpenBuffer` are imported via the existing `mod types;` declaration. No new use statement needed in `protocol.rs` if the existing scope already references them via path. If imports are needed at the top of `protocol.rs`, add:

```rust
use crate::types::{Active, OpenBuffer};
```

then change the response fields to bare `Active` / `Vec<OpenBuffer>`.

- [ ] **Step 6: Re-export `LineRange` from `lib.rs`**

Edit `helix-context-schema/src/lib.rs`. Extend the `pub use protocol::{...};` line to include `LineRange`:

```rust
pub use protocol::{
    ClientInfo, ControlRequest, ControlResponse, LineRange, ServerCapabilities, ServerInfo,
};
```

- [ ] **Step 7: Run all schema tests**

Run: `cargo test -p helix-context-schema`
Expected: All tests pass. Total: 23 (17 from Phase 2a + 6 new).

- [ ] **Step 8: Verify workspace still builds**

Run: `cargo check --workspace`
Expected: Clean. (helix-term will see the new variants but won't pattern-match on them yet — that's Task 4. Compile should still succeed because the existing `match` on `ControlRequest` in `dispatch.rs` only covers `Initialize`, but match `Some(_)` returns get caught by the outer `Option`.)

Actually wait — `dispatch::try_dispatch_inline` has `match request { ControlRequest::Initialize { .. } => Some(...) }`. Adding new variants without arms is a `non_exhaustive_patterns` error. So this WILL break compile — fix below.

- [ ] **Step 9: Stub the new variants in dispatch.rs**

Edit `helix-term/src/control_socket/dispatch.rs`. Find the existing match in `try_dispatch_inline`:

```rust
pub fn try_dispatch_inline(
    request: &ControlRequest,
) -> Option<Result<ControlResponse, JsonRpcError>> {
    match request {
        ControlRequest::Initialize {
            protocol_version,
            client_info,
        } => Some(handle_initialize(protocol_version, client_info)),
    }
}
```

Change it to:

```rust
pub fn try_dispatch_inline(
    request: &ControlRequest,
) -> Option<Result<ControlResponse, JsonRpcError>> {
    match request {
        ControlRequest::Initialize {
            protocol_version,
            client_info,
        } => Some(handle_initialize(protocol_version, client_info)),
        // CurrentState, GetOpenBuffers, GetBufferText all need &mut Editor.
        // Returning None routes them through the event-loop dispatch.
        ControlRequest::CurrentState {}
        | ControlRequest::GetOpenBuffers {}
        | ControlRequest::GetBufferText { .. } => None,
    }
}
```

- [ ] **Step 10: Verify workspace builds again**

Run: `cargo check --workspace`
Expected: Clean.

Run: `cargo test -p helix-term control_socket::dispatch`
Expected: 3 tests still pass.

- [ ] **Step 11: Commit**

```bash
git add helix-context-schema/src/protocol.rs helix-context-schema/src/lib.rs helix-context-schema/tests/protocol_roundtrip.rs helix-term/src/control_socket/dispatch.rs
git commit -m "$(cat <<'EOF'
feat(context-schema): add read-method protocol variants

ControlRequest: CurrentState {}, GetOpenBuffers {}, GetBufferText { path?, range? }.
ControlResponse: same variants with payloads (Active+mode, Vec<OpenBuffer>,
text+language+line_count). LineRange helper for the optional range parameter.

dispatch::try_dispatch_inline routes all three new variants to None (needs
editor state — handled by event-loop dispatch in Phase 2b Task 4).

Six new serde round-trip tests cover wire format, optional-field omission,
and full round-trip.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.2)
EOF
)"
```

---

## Task 2: Plumb the `UnboundedSender<EditorEvent>` from `Application` to `handle_connection`

**Files:**
- Modify: `helix-term/src/control_socket/server.rs`
- Modify: `helix-term/src/application.rs`

This task introduces the marshaling channel. After this task, the channel exists end-to-end but `handle_connection` still produces `MethodNotFound` for non-inline requests — replacing that with actual forwarding is Task 3.

- [ ] **Step 1: Modify `run_accept_loop` signature**

Edit `helix-term/src/control_socket/server.rs`. Find:

```rust
pub async fn run_accept_loop(listener: UnixListener) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                tokio::spawn(handle_connection(stream));
            }
            Err(e) => {
                log::warn!("control-socket: accept failed: {}", e);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}
```

Change to:

```rust
use tokio::sync::mpsc::UnboundedSender;
use helix_view::editor::EditorEvent;

pub async fn run_accept_loop(
    listener: UnixListener,
    control_tx: UnboundedSender<EditorEvent>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                tokio::spawn(handle_connection(stream, control_tx.clone()));
            }
            Err(e) => {
                log::warn!("control-socket: accept failed: {}", e);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}
```

Add `use tokio::sync::mpsc::UnboundedSender;` and `use helix_view::editor::EditorEvent;` at the top of the file if not already present.

- [ ] **Step 2: Modify `handle_connection` signature**

Find `async fn handle_connection(stream: tokio::net::UnixStream)`. Change to:

```rust
async fn handle_connection(
    stream: tokio::net::UnixStream,
    _control_tx: UnboundedSender<EditorEvent>,
) {
    // existing body — _control_tx is unused in this task, used in Task 3
```

The leading underscore on `_control_tx` silences the unused-variable warning until Task 3 wires it up.

- [ ] **Step 3: Modify the existing e2e test**

The test in `server.rs` calls `run_accept_loop(binding.listener)` and now needs to pass a sender. Find:

```rust
let server = tokio::spawn(run_accept_loop(binding.listener));
```

Change to:

```rust
let (control_tx, _control_rx) = tokio::sync::mpsc::unbounded_channel::<helix_view::editor::EditorEvent>();
let server = tokio::spawn(run_accept_loop(binding.listener, control_tx));
```

The `_control_rx` is dropped at end of test — that's fine since the test only exercises `initialize` (inline) and never forwards.

- [ ] **Step 4: Run server tests**

Run: `cargo test -p helix-term control_socket::server`
Expected: 1 test passes.

- [ ] **Step 5: Add `control_request_rx` field to `Application`**

Edit `helix-term/src/application.rs`. Find `pub struct Application { ... }`. Add a field after `control_socket_binding`:

```rust
    /// Receiver paired with the sender given to the control-socket accept
    /// loop. Receives `EditorEvent::ControlRequest` events from per-connection
    /// tasks; processed by `handle_control_request` in the main event loop.
    /// `None` when the control socket isn't enabled.
    control_request_rx:
        Option<tokio::sync::mpsc::UnboundedReceiver<helix_view::editor::EditorEvent>>,
```

- [ ] **Step 6: Update `start_control_socket` signature and body**

Find the helper function added in Phase 2a Task 9:

```rust
fn start_control_socket(
    editor: &helix_view::Editor,
) -> std::io::Result<crate::control_socket::path::Resolved> {
    // ...
    tokio::spawn(server::run_accept_loop(listener));
    Ok(resolved_for_cleanup)
}
```

Change the signature to take the sender as a parameter and pass it through:

```rust
fn start_control_socket(
    editor: &helix_view::Editor,
    control_tx: tokio::sync::mpsc::UnboundedSender<helix_view::editor::EditorEvent>,
) -> std::io::Result<crate::control_socket::path::Resolved> {
    use crate::control_socket::{lifecycle, path, server};

    let (workspace, is_cwd_fallback) = helix_loader::find_workspace();
    if is_cwd_fallback {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "no workspace marker — refusing to bind control socket in fallback directory",
        ));
    }

    let cfg = &editor.config().control_socket;
    let override_path = if cfg.path.as_os_str().is_empty() {
        None
    } else {
        Some(cfg.path.as_path())
    };

    let resolved = path::resolve_socket_path(&workspace, std::process::id(), override_path)?;
    let binding = lifecycle::bind_socket(resolved)?;
    let (listener, resolved_for_cleanup) = binding.split();

    tokio::spawn(server::run_accept_loop(listener, control_tx));

    Ok(resolved_for_cleanup)
}
```

- [ ] **Step 7: Update `Application::new` to create the channel and pass the sender**

Find the existing block:

```rust
        let control_socket_binding = if editor.config().control_socket.enabled {
            match start_control_socket(&editor) {
                Ok(resolved) => Some(resolved),
                Err(e) => {
                    log::warn!("control-socket: failed to start: {}", e);
                    None
                }
            }
        } else {
            None
        };
```

Change to:

```rust
        let (control_socket_binding, control_request_rx) = if editor.config().control_socket.enabled {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<helix_view::editor::EditorEvent>();
            match start_control_socket(&editor, tx) {
                Ok(resolved) => (Some(resolved), Some(rx)),
                Err(e) => {
                    log::warn!("control-socket: failed to start: {}", e);
                    (None, None)
                }
            }
        } else {
            (None, None)
        };
```

Then update the `Application { ... }` struct literal returned at the end of `new()` to include `control_request_rx,` alongside `control_socket_binding,`.

- [ ] **Step 8: Verify build**

Run: `cargo check --workspace`
Expected: Clean.

- [ ] **Step 9: Verify all tests still pass**

Run: `cargo test -p helix-context-schema`
Expected: 23 pass.

Run: `cargo test -p helix-term control_socket`
Expected: 13 pass.

- [ ] **Step 10: Commit**

```bash
git add helix-term/src/control_socket/server.rs helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): plumb UnboundedSender<EditorEvent> end-to-end

Application::new creates an mpsc::unbounded_channel<EditorEvent> when the
control socket is enabled. The sender flows down through
start_control_socket -> run_accept_loop -> handle_connection. The receiver
stays on Application as control_request_rx for the next task.

handle_connection accepts the sender but doesn't use it yet — Task 3 wires
the forward-and-await dispatch pattern. The intermediate state still
returns MethodNotFound for non-inline methods, but the channel is now in
place to be used.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§5.4)
EOF
)"
```

---

## Task 3: Wire `handle_connection` to forward non-inline requests

**Files:**
- Modify: `helix-term/src/control_socket/server.rs`

This task makes `handle_connection` actually forward `None`-from-`try_dispatch_inline` requests over the channel and await responses via `oneshot`. After this task, the channel ferries real traffic — but the receiving end (`Application::event_loop_until_idle`) doesn't have an arm yet, so non-inline requests will hang. Task 4 closes that loop.

- [ ] **Step 1: Replace the body of `handle_connection`**

Find the existing body (around line 30-65). Replace the inner loop's request-dispatch block. Before, it looked like:

```rust
        let resp = match try_dispatch_inline(&req) {
            Some(resp) => resp,
            None => Err(helix_context_schema::JsonRpcError {
                code: helix_context_schema::JsonRpcErrorCode::MethodNotFound,
                message: "method not available in Phase 2a".into(),
                data: None,
            }),
        };

        if let Err(e) = writer.write_response(&resp).await {
            log::warn!("control-socket: write error: {}", e);
            break;
        }
```

Replace with:

```rust
        let resp = match try_dispatch_inline(&req) {
            Some(resp) => resp,
            None => {
                // Forward into the main event loop via the channel.
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                let event = helix_view::editor::EditorEvent::ControlRequest {
                    request: req,
                    reply: reply_tx,
                };
                if _control_tx.send(event).is_err() {
                    // Receiver dropped — editor is shutting down.
                    log::warn!("control-socket: control_tx send failed; editor likely shutting down");
                    break;
                }
                match reply_rx.await {
                    Ok(r) => r,
                    Err(_) => {
                        // Sender dropped without sending — handler panicked or
                        // skipped the reply (shouldn't happen, but defensively).
                        Err(helix_context_schema::JsonRpcError {
                            code: helix_context_schema::JsonRpcErrorCode::InternalError,
                            message: "no reply from editor".into(),
                            data: None,
                        })
                    }
                }
            }
        };

        if let Err(e) = writer.write_response(&resp).await {
            log::warn!("control-socket: write error: {}", e);
            break;
        }
```

Also remove the underscore from `_control_tx` in the function signature — it's now used:

```rust
async fn handle_connection(
    stream: tokio::net::UnixStream,
    control_tx: UnboundedSender<EditorEvent>,
) {
```

And update the call in `run_accept_loop`:

```rust
tokio::spawn(handle_connection(stream, control_tx.clone()));
```

- [ ] **Step 2: Verify build**

Run: `cargo check --workspace`
Expected: Clean.

- [ ] **Step 3: Verify existing tests still pass**

Run: `cargo test -p helix-term control_socket`
Expected: 13 pass. The `initialize` happy-path test still works because it's an inline dispatch — `try_dispatch_inline` returns `Some(_)`, so the new forwarding code path is untouched.

- [ ] **Step 4: Commit**

```bash
git add helix-term/src/control_socket/server.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): forward non-inline requests via oneshot reply

When try_dispatch_inline returns None (i.e. the method needs &mut Editor),
handle_connection now packages the request with a oneshot::Sender as
EditorEvent::ControlRequest and sends it down the control_tx channel,
then awaits the reply.

The receiving end (Application::event_loop_until_idle) doesn't have a
matching select! arm yet, so non-inline requests will hang at the await.
Task 4 closes that loop.

If the channel send fails (editor shutting down) or the oneshot is
dropped without a value, surfaces InternalError to the client and
closes the connection.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§5.4)
EOF
)"
```

---

## Task 4: Receive in `event_loop_until_idle` + `handle_control_request` skeleton

**Files:**
- Modify: `helix-term/src/application.rs`

After this task, non-inline requests reach the editor event loop. Until specific method handlers are added (Tasks 5-7), `handle_control_request` returns `MethodNotFound` for all three new methods. That's the same wire result as before but now flowing through the real plumbing rather than the placeholder in `handle_connection`.

- [ ] **Step 1: Add the new `tokio::select!` arm**

Find `event_loop_until_idle` in `helix-term/src/application.rs` (around line 337). Locate the existing `tokio::select! { ... }` block. Inside it, after the existing arms, add:

```rust
                Some(event) = recv_control_request(&mut self.control_request_rx) => {
                    self.handle_control_request(event);
                }
```

Note: `recv_control_request` is a tiny helper because `Option::as_mut().recv()` doesn't compose cleanly with `select!`. Add this helper at the top level of `application.rs` (near `start_control_socket`):

```rust
/// Awaits a control request from the optional channel. If the channel is
/// `None` (control socket disabled) returns a future that never resolves —
/// so the `select!` arm is effectively disabled.
async fn recv_control_request(
    rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<helix_view::editor::EditorEvent>>,
) -> Option<helix_view::editor::EditorEvent> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}
```

- [ ] **Step 2: Add `handle_control_request` skeleton**

In `impl Application { ... }` (or wherever other handler methods live), add:

```rust
    fn handle_control_request(&mut self, event: helix_view::editor::EditorEvent) {
        use helix_context_schema::{
            ControlRequest, ControlResponse, JsonRpcError, JsonRpcErrorCode,
        };

        let helix_view::editor::EditorEvent::ControlRequest { request, reply } = event else {
            log::error!("control-socket: handle_control_request got non-ControlRequest event");
            return;
        };

        let resp: Result<ControlResponse, JsonRpcError> = match request {
            ControlRequest::Initialize { .. } => {
                // Shouldn't happen — Initialize is inline-dispatched.
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::InternalError,
                    message: "Initialize should be handled inline".into(),
                    data: None,
                })
            }
            ControlRequest::CurrentState {} => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "current-state handler not yet implemented".into(),
                    data: None,
                })
            }
            ControlRequest::GetOpenBuffers {} => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "get-open-buffers handler not yet implemented".into(),
                    data: None,
                })
            }
            ControlRequest::GetBufferText { .. } => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "get-buffer-text handler not yet implemented".into(),
                    data: None,
                })
            }
        };

        let _ = reply.send(resp);
    }
```

- [ ] **Step 3: Remove the dead stub arm from `handle_editor_event`**

Find the existing stub:

```rust
        EditorEvent::ControlRequest { request: _, reply } => {
            // TODO(control-socket): dispatch to the real handler once it is
            // implemented. For now, dropping `reply` signals InternalError
            // to the sender side via RecvError::Closed.
            log::warn!("ControlRequest received but handler not yet implemented; dropping reply");
            drop(reply);
        }
```

Replace with:

```rust
        EditorEvent::ControlRequest { reply, .. } => {
            // ControlRequest events are routed through control_request_rx
            // in event_loop_until_idle, not through editor.wait_event(). If
            // we ever hit this arm, the variant came from somewhere we didn't
            // expect — log and drop the reply (surfaces as InternalError on
            // the client).
            log::error!(
                "control-socket: ControlRequest reached handle_editor_event; \
                 this should not happen — please report",
            );
            drop(reply);
        }
```

- [ ] **Step 4: Verify build**

Run: `cargo check --workspace`
Expected: Clean.

- [ ] **Step 5: Verify existing tests still pass**

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 23 + 13 = 36 tests pass.

- [ ] **Step 6: Smoke-test the routing**

This needs a brief Helix run. Build the release binary:

```bash
cargo build --release -p helix-term --bin hx
```

Set up a test workspace and config (assuming you have `~/.config/helix/config.toml` with `[editor.control-socket] enabled = true` already; if not, add it temporarily):

```bash
mkdir -p /tmp/p2b-route && cd /tmp/p2b-route && git init -q && echo "fn main() {}" > main.rs
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 1.5
SOCK=$(ls /tmp/p2b-route/.helix/control-*.sock | head -1)
python3 -c "
import socket
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"current-state\",\"params\":{}}\n')
print(s.recv(8192).decode())
"
pkill -P $HX_PID
kill $HX_PID
rm -rf /tmp/p2b-route
```

Expected response: a `MethodNotFound` error with message `"current-state handler not yet implemented"`. This proves the request reached `handle_control_request` (i.e. the channel + select! arm work end-to-end).

- [ ] **Step 7: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): event-loop dispatch via control_request_rx

Adds a select! arm in event_loop_until_idle that awaits on
control_request_rx (via recv_control_request helper that returns
pending() when the channel is None). Dispatches to handle_control_request.

handle_control_request is currently a skeleton: it unpacks the
ControlRequest variant and returns MethodNotFound for the three
read-method variants. Subsequent tasks fill in real implementations.

Replaces the dead stub arm in handle_editor_event — ControlRequest
events are routed through their dedicated channel, not through
Editor::wait_event.

Smoke-tested: client gets "current-state handler not yet implemented"
proving the channel + dispatch path are wired correctly end-to-end.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§5.4)
EOF
)"
```

---

## Task 5: Implement `current-state` handler

**Files:**
- Modify: `helix-term/src/application.rs`

The `current-state` response includes the currently-focused buffer's full `Active` snapshot (path, language, cursors, selections) plus the editor mode. The data is exactly what `context_logger::build_snapshot` already produces, but we don't need the whole snapshot — just the `active` field + mode.

- [ ] **Step 1: Add a helper to build `Active` from current editor state**

We have two options:
- (A) Re-use `context_logger::build_snapshot` and extract `active` + `mode` from the result.
- (B) Build `Active` directly with a focused helper.

(A) is reusing tested code. (B) is faster and avoids the snapshot-source/timestamp overhead. Going with (A) — correctness > microseconds — but we need a public accessor for `build_snapshot` or extract a smaller helper.

Edit `helix-term/src/context_logger.rs`. Find `fn build_snapshot(...)` (private). Change `fn` to `pub(crate) fn`:

```rust
pub(crate) fn build_snapshot(
    editor: &Editor,
    workspace: &Path,
    cfg: &ContextLoggerConfig,
    source: UpdateSource,
) -> ContextSnapshot {
```

This is the minimum-surface change. The handler will pass a synthesized config + source.

- [ ] **Step 2: Replace the `CurrentState` stub in `handle_control_request`**

Edit `helix-term/src/application.rs`. Find the existing `ControlRequest::CurrentState {}` arm in `handle_control_request`. Replace with:

```rust
            ControlRequest::CurrentState {} => {
                let (workspace, is_cwd_fallback) = helix_loader::find_workspace();
                if is_cwd_fallback {
                    Err(JsonRpcError {
                        code: JsonRpcErrorCode::NoActiveDocument,
                        message: "no workspace marker — refusing to report state".into(),
                        data: None,
                    })
                } else {
                    let cfg = self.editor.config().context_logger.clone();
                    let snap = crate::context_logger::build_snapshot(
                        &self.editor,
                        &workspace,
                        &cfg,
                        helix_context_schema::UpdateSource::Manual,
                    );
                    Ok(ControlResponse::CurrentState {
                        active: snap.active,
                        mode: snap.mode,
                    })
                }
            }
```

- [ ] **Step 3: Verify build**

Run: `cargo check --workspace`
Expected: Clean. If you get an error about `Active`/`OpenBuffer` not being `Clone`, check that the types in `helix-context-schema/src/types.rs` derive `Clone` — they do per Phase 1.

- [ ] **Step 4: Verify existing tests still pass**

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 36 pass.

- [ ] **Step 5: Build release**

Run: `cargo build --release -p helix-term --bin hx`
Expected: Succeeds.

- [ ] **Step 6: Smoke-test `current-state`**

```bash
mkdir -p /tmp/p2b-cs && cd /tmp/p2b-cs && git init -q && echo "fn main() { let x = 1; }" > main.rs
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 1.5
SOCK=$(ls /tmp/p2b-cs/.helix/control-*.sock | head -1)
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"current-state\",\"params\":{}}\n')
resp = json.loads(s.recv(8192).decode())
print(json.dumps(resp, indent=2))
"
pkill -P $HX_PID
kill $HX_PID
rm -rf /tmp/p2b-cs
```

Expected output structure:
```json
{
  "method": "current-state",
  "result": {
    "active": {
      "path": "main.rs",
      "path_abs": "/private/tmp/p2b-cs/main.rs",
      "language": "rust",
      "modified": false,
      "line_count": 1,
      "cursors": [{"primary": true, "line": 1, "column": 1}],
      "selections": []
    },
    "mode": "normal"
  }
}
```

- [ ] **Step 7: Commit**

```bash
git add helix-term/src/context_logger.rs helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement current-state method

CurrentState handler reuses context_logger::build_snapshot (promoted from
private fn to pub(crate) fn) and extracts the .active and .mode fields.
This reuses Phase 1's tested cursor/selection/document mapping rather
than reimplementing it.

Returns NoActiveDocument error if launched outside a workspace marker.

Smoke-tested: returns full Active state matching the snapshot file format.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.2)
EOF
)"
```

---

## Task 6: Implement `get-open-buffers` handler

**Files:**
- Modify: `helix-term/src/application.rs`

Simpler than `current-state` — just iterate `editor.documents()` and produce `OpenBuffer` instances. The schema already defines `OpenBuffer { path, language, modified }` (Phase 1).

- [ ] **Step 1: Replace the `GetOpenBuffers` stub**

Edit `helix-term/src/application.rs`. Find the `ControlRequest::GetOpenBuffers {}` arm in `handle_control_request`. Replace with:

```rust
            ControlRequest::GetOpenBuffers {} => {
                let buffers: Vec<helix_context_schema::OpenBuffer> = self
                    .editor
                    .documents()
                    .map(|d| helix_context_schema::OpenBuffer {
                        path: d.path().map(|p| p.to_string_lossy().into_owned()),
                        language: d.language_name().map(|s| s.to_owned()),
                        modified: d.is_modified(),
                    })
                    .collect();
                Ok(ControlResponse::GetOpenBuffers { buffers })
            }
```

- [ ] **Step 2: Verify build**

Run: `cargo check --workspace`
Expected: Clean.

- [ ] **Step 3: Verify existing tests**

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 36 pass.

- [ ] **Step 4: Smoke-test**

```bash
mkdir -p /tmp/p2b-ob && cd /tmp/p2b-ob && git init -q
echo "fn main() {}" > main.rs && echo "// lib" > lib.rs
cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs lib.rs &
HX_PID=$!
sleep 1.5
SOCK=$(ls /tmp/p2b-ob/.helix/control-*.sock | head -1)
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"get-open-buffers\",\"params\":{}}\n')
print(json.dumps(json.loads(s.recv(8192).decode()), indent=2))
"
pkill -P $HX_PID
kill $HX_PID
rm -rf /tmp/p2b-ob
```

Expected output:
```json
{
  "method": "get-open-buffers",
  "result": {
    "buffers": [
      {"path": "/private/tmp/p2b-ob/main.rs", "language": "rust", "modified": false},
      {"path": "/private/tmp/p2b-ob/lib.rs", "language": "rust", "modified": false}
    ]
  }
}
```

(Order may vary; both buffers should appear.)

- [ ] **Step 5: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement get-open-buffers method

Iterates editor.documents() and maps each to helix_context_schema::OpenBuffer
(path, language, modified). Same mapping pattern as Phase 1's
context_logger snapshot.

Smoke-tested with two open buffers: both appear in the response.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.2)
EOF
)"
```

---

## Task 7: Implement `get-buffer-text` handler

**Files:**
- Modify: `helix-term/src/application.rs`

Most complex of the read methods because of the path resolution and optional range. Behavior:
- If `path` is `None`: use the active buffer.
- If `path` is `Some(p)` and `p` is absolute: find the buffer with that absolute path.
- If `path` is `Some(p)` and `p` is relative: resolve against the workspace root, then find the buffer.
- If no buffer matches: return `PathOutsideWorkspace` (or `NoActiveDocument` if no active buffer when path is None).
- If `range` is `None`: return the whole buffer.
- If `range` is `Some(r)`: return only those lines (1-indexed, inclusive).

- [ ] **Step 1: Add a helper to resolve a buffer by path**

Edit `helix-term/src/application.rs`. Add a private helper to `impl Application` (or as a top-level fn near `handle_control_request`):

```rust
/// Resolve a request's path to a (Document, language) pair. None path means
/// active buffer.
fn resolve_buffer<'a>(
    editor: &'a helix_view::Editor,
    workspace: &std::path::Path,
    path: Option<&str>,
) -> Result<&'a helix_view::Document, helix_context_schema::JsonRpcError> {
    use helix_context_schema::{JsonRpcError, JsonRpcErrorCode};

    match path {
        None => {
            let view = editor.tree.get(editor.tree.focus);
            editor.documents.get(&view.doc).ok_or(JsonRpcError {
                code: JsonRpcErrorCode::NoActiveDocument,
                message: "no active document".into(),
                data: None,
            })
        }
        Some(p) => {
            let p = std::path::Path::new(p);
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                workspace.join(p)
            };
            // Canonicalize for comparison robustness against `..` and symlinks.
            let abs_canon = std::fs::canonicalize(&abs).unwrap_or(abs.clone());
            editor
                .documents()
                .find(|d| {
                    d.path().map(|dp| {
                        std::fs::canonicalize(dp).unwrap_or_else(|_| dp.clone())
                    }) == Some(abs_canon.clone())
                })
                .ok_or(JsonRpcError {
                    code: JsonRpcErrorCode::PathOutsideWorkspace,
                    message: format!("no buffer open for path: {}", abs.display()),
                    data: None,
                })
        }
    }
}
```

- [ ] **Step 2: Replace the `GetBufferText` stub**

Find the `ControlRequest::GetBufferText { .. }` arm in `handle_control_request`. Replace with:

```rust
            ControlRequest::GetBufferText { path, range } => {
                let (workspace, _) = helix_loader::find_workspace();
                let doc_result = resolve_buffer(&self.editor, &workspace, path.as_deref());
                match doc_result {
                    Err(e) => Err(e),
                    Ok(doc) => {
                        let text = doc.text();
                        let extracted = if let Some(r) = range {
                            // Clamp the range to [1, line_count]
                            let start_line = r.start_line.saturating_sub(1).min(text.len_lines());
                            let end_line = r.end_line.min(text.len_lines());
                            if start_line >= end_line {
                                String::new()
                            } else {
                                let start_char = text.line_to_char(start_line);
                                // line_to_char returns the char index AT the start
                                // of the line; we want chars up to (but not including)
                                // the line after end_line. The char range is
                                // [start_char, end_char).
                                let end_char = if end_line >= text.len_lines() {
                                    text.len_chars()
                                } else {
                                    text.line_to_char(end_line)
                                };
                                text.slice(start_char..end_char).to_string()
                            }
                        } else {
                            text.to_string()
                        };
                        Ok(ControlResponse::GetBufferText {
                            text: extracted,
                            language: doc.language_name().map(|s| s.to_owned()),
                            line_count: text.len_lines(),
                        })
                    }
                }
            }
```

- [ ] **Step 3: Verify build**

Run: `cargo check --workspace`
Expected: Clean.

- [ ] **Step 4: Smoke-test (no range, active buffer)**

```bash
mkdir -p /tmp/p2b-gbt && cd /tmp/p2b-gbt && git init -q
cat > main.rs <<'EOF'
fn main() {
    println!("line 1");
    println!("line 2");
    println!("line 3");
    println!("line 4");
}
EOF
cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 1.5
SOCK=$(ls /tmp/p2b-gbt/.helix/control-*.sock | head -1)
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"get-buffer-text\",\"params\":{}}\n')
print(json.dumps(json.loads(s.recv(16384).decode()), indent=2))
"
```

Expected: full buffer text (6 lines), `line_count: 6`, `language: "rust"`.

- [ ] **Step 5: Smoke-test (with range)**

Without killing the prior hx:

```bash
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"get-buffer-text\",\"params\":{\"range\":{\"start_line\":2,\"end_line\":3}}}\n')
print(json.dumps(json.loads(s.recv(16384).decode()), indent=2))
"
```

Expected: `text` containing exactly the second and third lines (the two `println!` lines 1 and 2). Line endings included.

- [ ] **Step 6: Smoke-test (path to a non-open file)**

```bash
python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('$SOCK')
s.sendall(b'{\"method\":\"get-buffer-text\",\"params\":{\"path\":\"nonexistent.rs\"}}\n')
print(json.dumps(json.loads(s.recv(8192).decode()), indent=2))
"
pkill -P $HX_PID
kill $HX_PID
rm -rf /tmp/p2b-gbt
```

Expected: a `PathOutsideWorkspace` error (`code: -32005`).

- [ ] **Step 7: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement get-buffer-text method

Resolves the path (absolute or workspace-relative; None means active
buffer). Returns NoActiveDocument when None path and no buffer focused,
PathOutsideWorkspace when path doesn't match any open buffer.

When range is provided, extracts lines [start_line, end_line] (1-indexed,
inclusive). Range outside buffer bounds is clamped.

Smoke-tested: full-buffer fetch, range fetch (lines 2-3 of 6), error on
path to non-open file.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.2)
EOF
)"
```

---

## Task 8: Update `initialize` capabilities to advertise all read methods

**Files:**
- Modify: `helix-term/src/control_socket/dispatch.rs`
- Modify: `helix-context-schema/tests/protocol_roundtrip.rs`

- [ ] **Step 1: Update the failing test for advertised capabilities**

Edit `helix-context-schema/tests/protocol_roundtrip.rs`. Find `initialize_compatible_version_returns_ok` (Note: this is in `helix-term`, not `helix-context-schema` — see Step 2). Skip this step.

- [ ] **Step 2: Update the test in dispatch.rs**

Edit `helix-term/src/control_socket/dispatch.rs`. Find the existing test:

```rust
    #[test]
    fn initialize_compatible_version_returns_ok() {
        let req = ControlRequest::Initialize {
            protocol_version: "1.0".into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let resp = try_dispatch_inline(&req).unwrap().unwrap();
        let ControlResponse::Initialize { capabilities, .. } = resp;
        assert!(capabilities.read_methods.contains(&"initialize".to_string()));
    }
```

Update the assertion to check for all four read methods:

```rust
    #[test]
    fn initialize_advertises_all_phase_2b_read_methods() {
        let req = ControlRequest::Initialize {
            protocol_version: "1.0".into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let resp = try_dispatch_inline(&req).unwrap().unwrap();
        let ControlResponse::Initialize { capabilities, .. } = resp else {
            panic!("expected Initialize response");
        };
        let methods = &capabilities.read_methods;
        assert!(methods.contains(&"initialize".to_string()), "missing initialize");
        assert!(methods.contains(&"current-state".to_string()), "missing current-state");
        assert!(methods.contains(&"get-open-buffers".to_string()), "missing get-open-buffers");
        assert!(methods.contains(&"get-buffer-text".to_string()), "missing get-buffer-text");
        assert!(capabilities.write_methods.is_empty(), "write methods come in Phase 2c");
    }
```

Rename the test to `initialize_advertises_all_phase_2b_read_methods` (or keep the old name; the new assertion is what matters).

- [ ] **Step 3: Run, expect it to fail**

Run: `cargo test -p helix-term control_socket::dispatch`
Expected: 1 test fails — `current-state` etc. not in the capabilities list.

- [ ] **Step 4: Update `handle_initialize` to advertise the methods**

Edit `helix-term/src/control_socket/dispatch.rs`. Find `handle_initialize`. Update the `read_methods` Vec:

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

- [ ] **Step 5: Run tests**

Run: `cargo test -p helix-term control_socket`
Expected: 13 tests pass (the updated test now passes; the 2 other dispatch tests still pass).

- [ ] **Step 6: Final end-to-end smoke test (all four read methods)**

```bash
mkdir -p /tmp/p2b-final && cd /tmp/p2b-final && git init -q
cat > main.rs <<'EOF'
fn main() {
    let x = 1;
    let y = 2;
}
EOF
cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 1.5
SOCK=$(ls /tmp/p2b-final/.helix/control-*.sock | head -1)

python3 -c "
import socket, json
def call(method, params=None):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect('$SOCK')
    req = {'method': method, 'params': params or {}}
    s.sendall((json.dumps(req) + '\n').encode())
    return json.loads(s.recv(16384).decode())

for m in ['initialize', 'current-state', 'get-open-buffers', 'get-buffer-text']:
    p = {'protocol_version': '1.0', 'client_info': {'name': 't', 'version': '0.1'}} if m == 'initialize' else {}
    resp = call(m, p)
    print(f'=== {m} ===')
    print(json.dumps(resp, indent=2)[:500])
    print()
"

pkill -P $HX_PID
kill $HX_PID
rm -rf /tmp/p2b-final
```

Expected: All four methods return success responses (no `code: -32xxx` errors). `initialize` shows the four methods in `read_methods`.

- [ ] **Step 7: Commit**

```bash
git add helix-term/src/control_socket/dispatch.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): advertise all Phase 2b read methods in capabilities

handle_initialize.capabilities.read_methods now lists:
- initialize
- current-state
- get-open-buffers
- get-buffer-text

Test renamed and tightened to assert all four are present and that
write_methods is empty (Phase 2c will add open-file, goto-line).

Final smoke test: every method returns success for a real running hx
in a test workspace.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.1, §6.2)
EOF
)"
```

---

## Self-review checklist (for the implementer)

After all 8 tasks:

- [ ] `cargo check --workspace` clean
- [ ] `cargo test -p helix-context-schema` passes (23 tests: 17 prior + 6 new)
- [ ] `cargo test -p helix-term control_socket` passes (13 tests)
- [ ] `cargo build --release -p helix-term --bin hx` succeeds
- [ ] Manual smoke tests in Tasks 5-8 reproduce
- [ ] `git log --oneline -10` shows the 8 Phase 2b commits in clean sequence

## What's NOT in Phase 2b

- **State-mutating methods** (`open-file`, `goto-line`, `run-typable-command`) — Phase 2c
- **`write_context_file(Source::McpCommand)` after mutations** — Phase 2c
- **Populating `snapshot.instance`** — Phase 2c
- **LSP-backed methods** (`get-hover-at`, `get-definition-at`, etc.) — Phase 3
- **`helix-claude-mcp` external binary** — Phase 4

## Open questions for the implementer

1. **`current-state` reusing `build_snapshot`:** This pulls in the workspace lookup, timestamp generation, full open-buffer iteration — work the caller didn't ask for. If perf becomes an issue, extract a smaller `fn build_active(editor, view, doc) -> Active` helper. Not needed for Phase 2b's volume.

2. **`resolve_buffer` canonicalization:** Uses `std::fs::canonicalize` which fails for paths that don't exist. The fallback (`unwrap_or(abs)`) is correct but means a path the user typed that has a `..` won't normalize cleanly if the file doesn't exist. Edge case — fine for Phase 2b.

3. **`mpsc::unbounded_channel` choice:** Unbounded means a misbehaving client could OOM the editor by spamming requests. Phase 2c writes should bound the channel (e.g. capacity 64) and drop newest-first on full. For Phase 2b read methods this is unlikely to bite. Note in the followup queue.

4. **Concurrent connections:** Multiple clients hitting the socket simultaneously are fine — each gets its own `handle_connection` task with its own oneshot. Requests serialize at the editor event loop via the channel. No race, just throughput is bounded by the editor's main-thread processing.
