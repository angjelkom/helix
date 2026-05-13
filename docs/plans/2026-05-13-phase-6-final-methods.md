# Phase 6 — Final Methods (`format-document`, `run-command`, `:write-context`) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close out the spec. Add the three remaining methods: `format-document` (LSP-formatting via the same async-future pattern as Phase 3's LSP methods), `run-command` (execute any Helix typable command), and the `:write-context` typable command (the producer for `UpdateSource::Manual` that was declared in Phase 1 but never actually emitted). Plus the corresponding MCP Tools.

**Architecture:** `run-command` is the generic primitive: look up a `TypableCommand` by name in Helix's static command list, build a `compositor::Context` from `Application`'s fields, call the command's `fun` with the given args. `format-document` is a focused convenience — it does a path-switch then internally runs Helix's format machinery which produces LSP edits and applies them via the existing `cx.jobs.callback` pattern. The `:write-context` typable command calls `crate::context_logger::write_context_file(&editor, UpdateSource::Manual, instance)` so users can manually refresh the snapshot from inside Helix.

**Tech Stack:** Same as Phases 1-5. No new external deps.

**Spec:** `docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md` §6.3 (format-document, run-typable-command), §7.3 (helix_format_document, helix_run_command), §11 Phase 6 deliverables list.

---

## Project context for the implementer

You are working in a Helix editor fork at `/Users/angm/helix` on branch `nightly`. Phase 5 is complete (tip: `09a8221bf`, 80 commits ahead of remote). The control socket supports 11 methods; `helix-claude-mcp` exposes 3 Resources + 7 Tools + the Rust hook subcommand.

The three methods this phase adds, in priority order:

1. **`:write-context`** — easiest. Helix-side typable command that calls `write_context_file` with `UpdateSource::Manual`. Used by power users to force a snapshot refresh without switching panes. The `Manual` variant has been a "ghost" since Phase 1 — declared but never produced. This task gives it a producer.

2. **`run-command`** — Helix-side method that takes a typable-command name and args, looks up `TypableCommand` in `TYPABLE_COMMAND_LIST`, executes it. Powerful but predictable: it's literally "type this :command and press enter." The MCP tool surface is `helix_run_command` with a clear description warning about its power.

3. **`format-document`** — Helix-side method that, given an optional path, switches focus to that buffer then runs Helix's format machinery. Internally uses the same LSP-formatting → edit-application pattern that Helix's existing `commands::format` uses. The MCP tool is `helix_format_document`.

What Phase 6 does NOT do:
- `helix-claude-mcp doctor` self-diagnostic subcommand — deferred to a Phase 6b polish round if needed.
- `--verbose` flag for the hook + telemetry — deferred.
- `session_id` sanitization, dead-code-warning cleanup, additional fake-Helix integration tests for LSP methods — all minor Phase 5/4b review items deferred.

If the user wants to ship Phase 6 as the absolute end of the spec, those deferred items can land as a final cleanup commit; or they can be Phase 6b. This plan focuses on the user-facing functionality.

## File structure

**Modify:**

- `helix-context-schema/src/protocol.rs` — add `FormatDocument` and `RunCommand` to both `ControlRequest` and `ControlResponse`.
- `helix-context-schema/tests/protocol_roundtrip.rs` — round-trip tests for the new variants.
- `helix-term/src/control_socket/dispatch.rs` — route new variants to `None`; advertise both methods in `write_methods` capability.
- `helix-term/src/application.rs` — two new arms in `handle_control_request` (run-command, format-document).
- `helix-term/src/commands/typed.rs` — register `:write-context` typable command.
- `helix-claude-mcp/src/tools.rs` — `ToolKind::HelixFormatDocument` and `ToolKind::HelixRunCommand` + arg structs.
- `helix-claude-mcp/src/serve.rs` — two new arms in `call_tool`.
- `helix-claude-mcp/tests/integration.rs` — fake-Helix integration tests for the new tools.
- `helix-claude-mcp/README.md` — extend the tools table with the two new entries.

**No new files** — Phase 6 extends what's already in place.

## Type design

### New `ControlRequest` variants

```rust
FormatDocument {
    path: Option<String>,       // None = active buffer
},
RunCommand {
    name: String,               // typable command name without the leading ":"
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    args: Vec<String>,          // additional arguments, joined as the command's args string
},
```

### New `ControlResponse` variants

Both methods return a simple acknowledgment plus optional details:

```rust
FormatDocument {
    /// Whether the formatter ran. False when no LSP supports formatting
    /// for this language (already covered by NoLspForLanguage error but
    /// kept for symmetry).
    applied: bool,
},
RunCommand {
    /// Captured command output, if any. Most typable commands don't emit
    /// stdout — the editor's status line gets messages instead. We capture
    /// the status-line text when available.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    message: Option<String>,
},
```

### MCP Tool input schemas

```rust
HelixFormatDocument: {
    "type": "object",
    "properties": {
        "path": { "type": "string", "description": "Buffer path; defaults to active. Must already be open." }
    }
}

HelixRunCommand: {
    "type": "object",
    "properties": {
        "name": { "type": "string", "description": "Typable command name (no leading ':'). Examples: 'write', 'format', 'reload', 'open <path>'." },
        "args": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Optional arguments, joined into the command's argument string"
        }
    },
    "required": ["name"]
}
```

---

## Task 1: Schema additions for `FormatDocument` and `RunCommand`

**Files:**
- Modify: `helix-context-schema/src/protocol.rs`
- Modify: `helix-context-schema/tests/protocol_roundtrip.rs`
- Modify: `helix-term/src/control_socket/dispatch.rs`
- Modify: `helix-term/src/application.rs`

- [ ] **Step 1: Write failing tests**

Append to `helix-context-schema/tests/protocol_roundtrip.rs`:

```rust
#[test]
fn format_document_request_with_optional_path() {
    let req = ControlRequest::FormatDocument { path: Some("src/main.rs".into()) };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "format-document");
    assert_eq!(j["params"]["path"], "src/main.rs");
}

#[test]
fn format_document_request_with_no_path_omits_field() {
    let req = ControlRequest::FormatDocument { path: None };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "format-document");
    assert!(j["params"].get("path").is_none() || j["params"]["path"].is_null());
}

#[test]
fn format_document_response_round_trips() {
    let resp = ControlResponse::FormatDocument { applied: true };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["method"], "format-document");
    assert_eq!(j["result"]["applied"], true);
    let back: ControlResponse = serde_json::from_value(j).unwrap();
    let ControlResponse::FormatDocument { applied } = back else {
        panic!("wrong variant");
    };
    assert!(applied);
}

#[test]
fn run_command_request_with_no_args_omits_field() {
    let req = ControlRequest::RunCommand { name: "write".into(), args: vec![] };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["method"], "run-command");
    assert_eq!(j["params"]["name"], "write");
    assert!(j["params"].get("args").is_none() || j["params"]["args"].as_array().map(|a| a.is_empty()).unwrap_or(true));
}

#[test]
fn run_command_request_with_args() {
    let req = ControlRequest::RunCommand {
        name: "open".into(),
        args: vec!["src/main.rs".into()],
    };
    let j = serde_json::to_value(&req).unwrap();
    assert_eq!(j["params"]["args"][0], "src/main.rs");
}

#[test]
fn run_command_response_with_message_round_trips() {
    let resp = ControlResponse::RunCommand {
        message: Some("written 5 buffers".into()),
    };
    let j = serde_json::to_value(&resp).unwrap();
    assert_eq!(j["result"]["message"], "written 5 buffers");
}

#[test]
fn run_command_response_with_no_message_omits_field() {
    let resp = ControlResponse::RunCommand { message: None };
    let j = serde_json::to_value(&resp).unwrap();
    assert!(j["result"].get("message").is_none() || j["result"]["message"].is_null());
}
```

- [ ] **Step 2: Run, confirm failure**

Run: `cargo test -p helix-context-schema`
Expected: 7 new tests fail — variants don't exist.

- [ ] **Step 3: Add variants to ControlRequest and ControlResponse**

Edit `helix-context-schema/src/protocol.rs`. Extend `ControlRequest` (add after `GetWorkspaceSymbols`):

```rust
    FormatDocument {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        path: Option<String>,
    },
    RunCommand {
        name: String,
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        args: Vec<String>,
    },
```

Extend `ControlResponse` (add after `GetWorkspaceSymbols`):

```rust
    FormatDocument {
        applied: bool,
    },
    RunCommand {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        message: Option<String>,
    },
```

- [ ] **Step 4: Stub the new variants in dispatch.rs**

Edit `helix-term/src/control_socket/dispatch.rs`. Find `try_dispatch_inline`'s `|` chain. Extend to include the new variants returning None:

```rust
        ControlRequest::CurrentState {}
        | ControlRequest::GetOpenBuffers {}
        | ControlRequest::GetBufferText { .. }
        | ControlRequest::OpenFile { .. }
        | ControlRequest::GotoLine { .. }
        | ControlRequest::GetDiagnostics { .. }
        | ControlRequest::GetHoverAt { .. }
        | ControlRequest::GetDefinitionAt { .. }
        | ControlRequest::GetReferencesAt { .. }
        | ControlRequest::GetWorkspaceSymbols { .. }
        | ControlRequest::FormatDocument { .. }
        | ControlRequest::RunCommand { .. } => None,
```

- [ ] **Step 5: Stub the new arms in `handle_control_request`**

Edit `helix-term/src/application.rs`. Add MethodNotFound stubs in `handle_control_request`. Place near the other variant arms (Tasks 3-4 will replace these stubs):

```rust
            ControlRequest::FormatDocument { .. } => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "format-document handler not yet implemented".into(),
                    data: None,
                })
            }
            ControlRequest::RunCommand { .. } => {
                Err(JsonRpcError {
                    code: JsonRpcErrorCode::MethodNotFound,
                    message: "run-command handler not yet implemented".into(),
                    data: None,
                })
            }
```

- [ ] **Step 6: Verify + tests**

Run: `cargo check --workspace`
Expected: Clean.

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: 44 prior + 7 new = 51 schema tests pass. 15 control_socket tests pass.

- [ ] **Step 7: Commit**

```bash
git add helix-context-schema/src/protocol.rs helix-context-schema/tests/protocol_roundtrip.rs helix-term/src/control_socket/dispatch.rs helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(context-schema): add FormatDocument and RunCommand variants

Two new ControlRequest variants:
- FormatDocument { path? } — run LSP formatter on a buffer
- RunCommand { name, args } — execute any Helix typable command

Matching ControlResponse variants with minimal payloads (applied: bool
for format, optional message string for run-command — captures the
status-line text when the command emits one).

dispatch.rs routes both to None (event-loop handling). application.rs
returns MethodNotFound stubs — real handlers in Tasks 3-4.

Seven new serde round-trip tests cover wire format and optional-field
omission for both request and response variants.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.3)
EOF
)"
```

---

## Task 2: `:write-context` typable command

**Files:**
- Modify: `helix-term/src/commands/typed.rs`

This is the smallest task. `:write-context` is a user-facing Helix command that calls `context_logger::write_context_file(editor, UpdateSource::Manual, None)`. Until Phase 6, `UpdateSource::Manual` has been a ghost variant — declared in `helix-context-schema` but never produced. This task gives it a producer.

- [ ] **Step 1: Inspect the typable command registration pattern**

Run: `grep -n "TypableCommand {" /Users/angm/helix/helix-term/src/commands/typed.rs | head -5`

Pick an existing simple command (e.g., one that just calls a function and returns Ok). Use it as a template.

Also find:
```bash
grep -n "TYPABLE_COMMAND_LIST" /Users/angm/helix/helix-term/src/commands/typed.rs | head -3
```

The static list is where you register new commands.

- [ ] **Step 2: Write the command function**

Edit `helix-term/src/commands/typed.rs`. Find a logical place near other write-related commands (after `write_all_impl` or similar). Add:

```rust
fn write_context(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    if let Err(e) = crate::context_logger::write_context_file(
        cx.editor,
        helix_context_schema::UpdateSource::Manual,
        None,
    ) {
        cx.editor.set_error(format!("write-context: {}", e));
        return Err(anyhow::anyhow!("write-context failed: {}", e));
    }
    cx.editor.set_status("context snapshot written");
    Ok(())
}
```

The exact signature comes from the project's existing typable commands — adjust if the function signature differs (e.g., if `Args` is different in this fork).

- [ ] **Step 3: Register the command in `TYPABLE_COMMAND_LIST`**

Find `TYPABLE_COMMAND_LIST`. Add a new entry, alphabetical-ish near other `write-` entries:

```rust
    TypableCommand {
        name: "write-context",
        aliases: &[],
        doc: "Write the context-logger snapshot to disk. Useful to force a refresh \
              for an external tool (e.g. Claude Code) without switching terminal panes.",
        fun: write_context,
        completer: CommandCompleter::none(),
        signature: Signature {
            positionals: (0, Some(0)),
            ..Default::default()
        },
    },
```

The exact `Signature`/`CommandCompleter` field names depend on this fork's existing code. Match the simplest existing command's structure (e.g. `:reload` or `:redraw`).

- [ ] **Step 4: Verify build**

Run: `cargo check --workspace`
Expected: Clean. If the signature shape is wrong, iterate — the existing commands tell you what's expected.

- [ ] **Step 5: Run existing tests (no new tests needed for the typable command itself)**

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: All still pass.

- [ ] **Step 6: Smoke-test the command interactively**

Build release:
```bash
cargo build --release -p helix-term --bin hx
```

Set up:
```bash
mkdir -p /tmp/p6-wc && cd /tmp/p6-wc && git init -q && echo "fn main() {}" > main.rs
```

Make sure `~/.config/helix/config.toml` has `[editor.context-logger] enabled = true`.

Start Helix:
```bash
script -q /dev/null /Users/angm/helix/target/release/hx /tmp/p6-wc/main.rs &
HX_PID=$!
sleep 1.5
```

If the sandbox can't run hx interactively, just verify the snapshot is created on focus-loss as before, and rely on the integration test the next task adds.

If you have an interactive shell, type `:write-context<enter>` in Helix. Expected:
- Status line shows "context snapshot written"
- File at `/tmp/p6-wc/.helix/context.json` exists with `"last_update_source": "manual"`

Cleanup:
```bash
pkill -P $HX_PID 2>/dev/null
kill $HX_PID 2>/dev/null
sleep 0.5
rm -rf /tmp/p6-wc
```

If interactive testing isn't possible, just verify the source compiles + commits cleanly. The Phase 6 final smoke test will exercise this through `helix_run_command("write-context")`.

- [ ] **Step 7: Commit**

```bash
git add helix-term/src/commands/typed.rs
git commit -m "$(cat <<'EOF'
feat(commands): add :write-context typable command

Calls context_logger::write_context_file(editor, UpdateSource::Manual,
None). Lets users force a snapshot refresh from inside Helix — useful
when an external tool needs fresh state and you don't want to switch
terminal panes to trigger focus_lost.

UpdateSource::Manual has been a ghost variant since Phase 1: declared
but never produced. This commit closes the loop.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§11
Phase 6 deliverables)
EOF
)"
```

---

## Task 3: `run-command` Helix-side handler

**Files:**
- Modify: `helix-term/src/application.rs`

The handler:
1. Look up the named command in `TYPABLE_COMMAND_LIST`.
2. Build an `Args` value from the args strings (Helix's args type takes a `&str`; we join the args with spaces).
3. Construct a `compositor::Context { editor: &mut self.editor, scroll: None, jobs: &mut self.jobs }`.
4. Call the command's `fun(&mut cx, args, PromptEvent::Validate)`.
5. On success: capture any status-line text from `editor.get_last_status()` (or similar). Return `Ok(ControlResponse::RunCommand { message })`.
6. On failure: map the error to `JsonRpcError::InternalError`.

- [ ] **Step 1: Inspect `TypableCommand` and `Args`**

Run: `grep -n "pub struct Args\|impl Args\|TypableCommand::find\|TYPABLE_COMMAND_MAP" /Users/angm/helix/helix-term/src/commands/typed.rs | head -10`

You're looking for:
- The shape of `Args` (parsed from a string)
- A lookup function or static map for finding a command by name
- The signature of the command's `fun`

If there's no `find_by_name`, iterate `TYPABLE_COMMAND_LIST` linearly — it has at most a few hundred entries.

Also check `PromptEvent`:
```bash
grep -n "PromptEvent" /Users/angm/helix/helix-term/src/keymap/macros.rs /Users/angm/helix/helix-view/src/input.rs 2>/dev/null | head -5
```

The variants are `Validate`, `Update`, `Abort`. We use `Validate`.

- [ ] **Step 2: Replace the `RunCommand` stub**

Edit `helix-term/src/application.rs`. Find the `ControlRequest::RunCommand { .. }` stub in `handle_control_request`. Replace with:

```rust
            ControlRequest::RunCommand { name, args } => {
                use crate::commands::typed::TYPABLE_COMMAND_LIST;
                use crate::compositor;
                use crate::ui::PromptEvent;

                let Some(cmd) = TYPABLE_COMMAND_LIST.iter().find(|c| c.name == name) else {
                    let _ = reply.send(Err(JsonRpcError {
                        code: JsonRpcErrorCode::InvalidParams,
                        message: format!("unknown typable command: {}", name),
                        data: None,
                    }));
                    return;
                };

                let args_str = args.join(" ");
                let args_parsed = match crate::commands::Args::parse(
                    &args_str,
                    cmd.signature,
                    true,    // validate=true; matches what `:cmd args` does
                    |_| Ok(""),  // expand callback — empty for our purposes
                ) {
                    Ok(a) => a,
                    Err(e) => {
                        let _ = reply.send(Err(JsonRpcError {
                            code: JsonRpcErrorCode::InvalidParams,
                            message: format!("invalid args for {}: {}", name, e),
                            data: None,
                        }));
                        return;
                    }
                };

                let mut cx = compositor::Context {
                    editor: &mut self.editor,
                    scroll: None,
                    jobs: &mut self.jobs,
                };

                let result = (cmd.fun)(&mut cx, args_parsed, PromptEvent::Validate);

                // Capture the editor's last status message, if any.
                let message = self
                    .editor
                    .get_status()
                    .map(|(text, _severity)| text.to_string());

                match result {
                    Ok(()) => {
                        // Rewrite the snapshot — run-command may have mutated state.
                        let instance = self.control_socket_binding.as_ref().map(|s| s.to_instance());
                        if let Err(e) = crate::context_logger::write_context_file(
                            &self.editor,
                            helix_context_schema::UpdateSource::McpCommand,
                            instance,
                        ) {
                            log::warn!("control-socket: snapshot rewrite failed after run-command: {}", e);
                        }
                        let _ = reply.send(Ok(ControlResponse::RunCommand { message }));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(JsonRpcError {
                            code: JsonRpcErrorCode::InternalError,
                            message: format!("command '{}' failed: {}", name, e),
                            data: None,
                        }));
                    }
                }
                return;
            }
```

The `Args::parse` call signature is the most likely place to need adjustment. Inspect the actual signature with `grep -n "impl Args\|pub fn parse" /Users/angm/helix/helix-term/src/commands/typed.rs` (or wherever Args lives). Adapt the parse call to what the type actually accepts.

The `editor.get_status()` method's exact name varies — common names: `get_status`, `last_status`, `status_line_message`. Find it:
```bash
grep -n "fn get_status\|fn status\|status_text" /Users/angm/helix/helix-view/src/editor.rs | head -10
```

If there's no convenient getter, return `message: None`.

- [ ] **Step 3: Verify build**

Run: `cargo check --workspace`
Expected: Clean after API adjustments.

- [ ] **Step 4: Tests pass**

Run: `cargo test -p helix-context-schema && cargo test -p helix-term control_socket`
Expected: All pass.

- [ ] **Step 5: Smoke test (Helix-side via socket)**

If possible, exercise run-command end-to-end via a Python client:

```bash
mkdir -p /tmp/p6-runcmd && cd /tmp/p6-runcmd && git init -q
echo "fn main() {}" > main.rs

cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 1.5
SOCK=$(ls /tmp/p6-runcmd/.helix/control-*.sock | head -1)

python3 -c "
import socket, json
def call(req):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect('$SOCK')
    s.sendall((json.dumps(req) + '\n').encode())
    return json.loads(s.recv(8192).decode())

# write-context
print(call({'method':'run-command','params':{'name':'write-context'}}))
# split-vertical (a side-effect-only command)
print(call({'method':'run-command','params':{'name':'split-vertical'}}))
# unknown command
print(call({'method':'run-command','params':{'name':'definitely-not-a-command'}}))
"

pkill -P $HX_PID; kill $HX_PID
rm -rf /tmp/p6-runcmd
```

Expected:
- `write-context` → `{"method":"run-command","result":{"message":"context snapshot written"}}` (or similar)
- `split-vertical` → `{"method":"run-command","result":{}}` (no message)
- `definitely-not-a-command` → JsonRpcError `-32602 unknown typable command: definitely-not-a-command`

Cleanup, commit.

- [ ] **Step 6: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement run-command method

Looks up the requested name in TYPABLE_COMMAND_LIST. If not found,
returns InvalidParams with the bad name. Otherwise constructs an Args
from the joined arg strings, builds a compositor::Context from
&mut self.editor and &mut self.jobs, and invokes the command's fun
with PromptEvent::Validate.

On success: captures the editor's last status-line message (if any)
into the response's message field. Rewrites the snapshot tagged
mcp_command since run-command may mutate state.

On command failure: maps to InternalError with the command name and
underlying error.

Smoke-tested with write-context (status message captured), split-vertical
(no message), and an unknown command (InvalidParams error).

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.3)
EOF
)"
```

---

## Task 4: `format-document` Helix-side handler

**Files:**
- Modify: `helix-term/src/application.rs`

`format-document` is conceptually a thin wrapper: switch to the buffer at `path` (if given) then run Helix's format machinery. The simplest implementation delegates to the existing typable `:format` command via the same mechanics as Task 3's run-command — but with the path-switch baked in.

- [ ] **Step 1: Inspect Helix's existing format command**

Run: `grep -n "fn format\|\"format\"" /Users/angm/helix/helix-term/src/commands/typed.rs | head -10`

You'll find the typable `:format` command. Note its name (probably `"format"`).

Also check whether Helix's `Document` has a format method that returns a Future:
```bash
grep -n "pub fn format\|fn format(" /Users/angm/helix/helix-view/src/document.rs | head -5
```

- [ ] **Step 2: Replace the `FormatDocument` stub**

The cleanest implementation: switch to the target buffer (if path given), then invoke the typable `:format` command using the same mechanics as run-command.

```rust
            ControlRequest::FormatDocument { path } => {
                use crate::commands::typed::TYPABLE_COMMAND_LIST;
                use crate::compositor;
                use crate::ui::PromptEvent;

                let (workspace, _) = helix_loader::find_workspace();
                if let Some(p) = path.as_deref() {
                    let doc = match resolve_buffer(&self.editor, &workspace, Some(p)) {
                        Ok(d) => d,
                        Err(e) => {
                            let _ = reply.send(Err(e));
                            return;
                        }
                    };
                    let doc_id = doc.id();
                    let current_doc_id = self
                        .editor
                        .documents
                        .get(&self.editor.tree.get(self.editor.tree.focus).doc)
                        .map(|d| d.id());
                    if Some(doc_id) != current_doc_id {
                        self.editor.switch(doc_id, helix_view::editor::Action::Replace);
                    }
                }

                // Find the :format typable command.
                let Some(cmd) = TYPABLE_COMMAND_LIST.iter().find(|c| c.name == "format") else {
                    let _ = reply.send(Err(JsonRpcError {
                        code: JsonRpcErrorCode::InternalError,
                        message: "internal: typable command 'format' not found in TYPABLE_COMMAND_LIST".into(),
                        data: None,
                    }));
                    return;
                };

                // Empty Args.
                let args_parsed = match crate::commands::Args::parse(
                    "",
                    cmd.signature,
                    true,
                    |_| Ok(""),
                ) {
                    Ok(a) => a,
                    Err(e) => {
                        let _ = reply.send(Err(JsonRpcError {
                            code: JsonRpcErrorCode::InternalError,
                            message: format!("format args parse failed: {}", e),
                            data: None,
                        }));
                        return;
                    }
                };

                let mut cx = compositor::Context {
                    editor: &mut self.editor,
                    scroll: None,
                    jobs: &mut self.jobs,
                };
                let applied = (cmd.fun)(&mut cx, args_parsed, PromptEvent::Validate).is_ok();

                // Snapshot rewrite (formatting mutates the buffer).
                let instance = self.control_socket_binding.as_ref().map(|s| s.to_instance());
                if let Err(e) = crate::context_logger::write_context_file(
                    &self.editor,
                    helix_context_schema::UpdateSource::McpCommand,
                    instance,
                ) {
                    log::warn!("control-socket: snapshot rewrite failed after format-document: {}", e);
                }

                let _ = reply.send(Ok(ControlResponse::FormatDocument { applied }));
                return;
            }
```

Note: `:format` uses `cx.jobs.callback(...)` to apply the LSP formatting edits asynchronously. The callback runs later when the LSP responds. From the caller's perspective, our response says `applied: true` but the actual edit hasn't been applied yet — it's queued in jobs.

This is the same model Helix's interactive `:format` uses. The status line eventually shows the result. For Phase 6, this is acceptable: the response says "we started the format"; the snapshot will reflect the formatted state after the LSP responds.

If you want strict guarantees ("response only after edits applied"), you'd need to await the format future before responding. That's a significantly bigger change — defer to a future polish phase if needed.

- [ ] **Step 3: Verify build**

Run: `cargo check --workspace`
Expected: Clean.

- [ ] **Step 4: Smoke test**

```bash
mkdir -p /tmp/p6-format && cd /tmp/p6-format && git init -q
cat > Cargo.toml <<'EOF'
[package]
name = "p6-format"
version = "0.1.0"
edition = "2021"
[[bin]]
name = "main"
path = "main.rs"
EOF
cat > main.rs <<'EOF'
fn   main()  {  let x=1;println!("{}",x); }
EOF

cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 6  # rust-analyzer cold start
SOCK=$(ls /tmp/p6-format/.helix/control-*.sock | head -1)

python3 -c "
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect('$SOCK')
s.sendall(b'{\"method\":\"format-document\",\"params\":{}}\n')
print(json.loads(s.recv(8192).decode()))
"

sleep 1   # let the LSP format callback run
cat /tmp/p6-format/main.rs

pkill -P $HX_PID; kill $HX_PID
rm -rf /tmp/p6-format
```

Expected:
- format-document response: `{"method":"format-document","result":{"applied":true}}`
- main.rs after sleep: formatted by rustfmt (proper spacing, indentation)

- [ ] **Step 5: Commit**

```bash
git add helix-term/src/application.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): implement format-document method

Switches focus to the requested buffer (if path given) then invokes
the :format typable command via the same TYPABLE_COMMAND_LIST lookup
+ Args parse + fun() execution pattern that run-command uses.

Returns applied: true if the :format command returned Ok. Note that
applied: true means "the format was kicked off" — the LSP response
arrives asynchronously and edits are applied via the standard Helix
jobs callback mechanism. The snapshot rewrite captures the eventual
mutated state on the next focus_lost/MCP call.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.3)
EOF
)"
```

---

## Task 5: Advertise the new methods + integration tests

**Files:**
- Modify: `helix-term/src/control_socket/dispatch.rs`

- [ ] **Step 1: Update the test for advertised capabilities**

Find the test `initialize_advertises_phase_2c_write_methods` in `helix-term/src/control_socket/dispatch.rs`. Rename it (e.g. `initialize_advertises_all_write_methods`) and update the assertions to require all 4 write methods now: `open-file`, `goto-line`, `format-document`, `run-command`.

```rust
    #[test]
    fn initialize_advertises_all_write_methods() {
        let req = ControlRequest::Initialize {
            protocol_version: "1.0".into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let resp = try_dispatch_inline(&req).unwrap().unwrap();
        let ControlResponse::Initialize { capabilities, .. } = resp else {
            panic!("expected Initialize response");
        };
        let writes = &capabilities.write_methods;
        for m in &["open-file", "goto-line", "format-document", "run-command"] {
            assert!(writes.contains(&m.to_string()), "missing write method: {}", m);
        }
    }
```

- [ ] **Step 2: Run, confirm failure**

Run: `cargo test -p helix-term control_socket::dispatch`
Expected: New test fails (write_methods doesn't yet include format-document or run-command).

- [ ] **Step 3: Update `handle_initialize` capabilities**

Find `handle_initialize` in `helix-term/src/control_socket/dispatch.rs`. Update `write_methods`:

```rust
            write_methods: vec![
                "open-file".into(),
                "goto-line".into(),
                "format-document".into(),
                "run-command".into(),
            ],
```

- [ ] **Step 4: Tests pass**

Run: `cargo test -p helix-term control_socket`
Expected: 15 pass.

- [ ] **Step 5: Final Helix-side end-to-end smoke**

Run all 13 socket methods (initialize + 9 read + 4 write) against a live Helix to verify nothing regressed:

```bash
mkdir -p /tmp/p6-final-hx && cd /tmp/p6-final-hx && git init -q
cat > Cargo.toml <<'EOF'
[package]
name = "p6-final-hx"
version = "0.1.0"
edition = "2021"
[[bin]]
name = "main"
path = "main.rs"
EOF
cat > main.rs <<'EOF'
fn   helper()->u32{42}
fn main(){let x=helper();println!("{}",x);}
EOF

cargo build --release -p helix-term --bin hx
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 6
SOCK=$(ls /tmp/p6-final-hx/.helix/control-*.sock | head -1)

python3 <<PYEOF
import socket, json
def call(method, params=None):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect('$SOCK')
    s.sendall((json.dumps({'method':method,'params':params or {}}) + '\n').encode())
    return json.loads(s.recv(16384).decode())

for m, p in [
    ('initialize', {'protocol_version':'1.0','client_info':{'name':'t','version':'0.1'}}),
    ('current-state', {}),
    ('get-open-buffers', {}),
    ('get-buffer-text', {}),
    ('get-diagnostics', {}),
    ('get-hover-at', {'line':2,'column':14}),
    ('get-definition-at', {'line':2,'column':14}),
    ('get-references-at', {'line':1,'column':4}),
    ('get-workspace-symbols', {'query':'helper'}),
    ('open-file', {'path':'main.rs'}),
    ('goto-line', {'line':1}),
    ('run-command', {'name':'write-context'}),
    ('format-document', {}),
]:
    r = call(m, p)
    print(f'{m}: {"OK" if "result" in r else "ERR"} {json.dumps(r)[:100]}')
PYEOF

pkill -P $HX_PID; kill $HX_PID
rm -rf /tmp/p6-final-hx
```

Expected: every method line shows `OK` (no `ERR`). format-document and run-command return Ok results. After sleep, the formatted main.rs is on disk.

- [ ] **Step 6: Commit**

```bash
git add helix-term/src/control_socket/dispatch.rs
git commit -m "$(cat <<'EOF'
feat(control-socket): advertise format-document and run-command

initialize.capabilities.write_methods grows from 2 to 4: adds
format-document and run-command alongside the existing open-file and
goto-line.

initialize_advertises_phase_2c_write_methods renamed to
initialize_advertises_all_write_methods and tightened to require all
four names present.

Final Helix-side smoke covers all 13 socket methods against a live
hx + rust-analyzer; every method returns a non-error response.

Phase 6 Helix-side work complete. MCP-side tools follow in Task 6.

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§6.1, §6.3)
EOF
)"
```

---

## Task 6: MCP-side tools — `helix_format_document` and `helix_run_command`

**Files:**
- Modify: `helix-claude-mcp/src/tools.rs`
- Modify: `helix-claude-mcp/src/serve.rs`

- [ ] **Step 1: Add ToolKind variants and arg structs**

Edit `helix-claude-mcp/src/tools.rs`. Extend `ToolKind`:

```rust
pub enum ToolKind {
    HelixOpenFile,
    HelixGotoLine,
    HelixGetDiagnostics,
    HelixGetHover,
    HelixGetDefinition,
    HelixGetReferences,
    HelixGetWorkspaceSymbols,
    HelixFormatDocument,
    HelixRunCommand,
}
```

Update each `match self {...}` arm to cover the new variants:

```rust
            Self::HelixFormatDocument => "helix_format_document",
            Self::HelixRunCommand => "helix_run_command",
```

```rust
            Self::HelixFormatDocument => {
                "Format a buffer using the LSP formatter (rust-analyzer, gopls, etc.). \
                 The buffer must already be open; pass path: 'foo.rs' to format a specific \
                 buffer, or omit path to format the active one. Returns applied: true when \
                 the format was kicked off. The actual edits arrive asynchronously via the \
                 LSP response."
            }
            Self::HelixRunCommand => {
                "Execute an arbitrary Helix typable command. POWERFUL — can do anything a \
                 user can type at the `:` prompt: write files, reload config, run shell \
                 commands via `:run-shell-command`, etc. Pass name as the command without \
                 the leading `:`. Use args for additional arguments (joined with spaces). \
                 Examples: { name: 'write' } to save; { name: 'reload' } to reload from \
                 disk; { name: 'open', args: ['src/main.rs'] } to open a file."
            }
```

```rust
            Self::HelixFormatDocument => json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Buffer path (optional; defaults to active)" }
                }
            }),
            Self::HelixRunCommand => json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Typable command name without leading ':'" },
                    "args": { "type": "array", "items": { "type": "string" }, "description": "Optional command arguments" }
                },
                "required": ["name"]
            }),
```

```rust
            "helix_format_document" => Some(Self::HelixFormatDocument),
            "helix_run_command" => Some(Self::HelixRunCommand),
```

Update `all()` to include both new variants.

Add the args structs:

```rust
#[derive(Debug, Deserialize)]
pub struct HelixFormatDocumentArgs {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HelixRunCommandArgs {
    pub name: String,
    #[serde(default)]
    pub args: Vec<String>,
}
```

- [ ] **Step 2: Update the tests in tools.rs**

Find the test `all_iterates_seven_kinds` — rename to `all_iterates_nine_kinds` (or whichever count is right after the additions) and update the asserted count.

Find `from_name_round_trips_with_name` — should still pass since it iterates `all()`.

Find `input_schema_is_an_object_schema` — still passes (the helpers cover all variants now).

- [ ] **Step 3: Add dispatch arms in serve.rs**

Edit `helix-claude-mcp/src/serve.rs`. Find the `call_tool` match. Add arms for the new variants (the match should still be exhaustive after these are added):

```rust
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
```

- [ ] **Step 4: Verify build + tests**

Run: `cargo check --workspace`
Expected: Clean.

Run: `cargo test -p helix-claude-mcp`
Expected: All tests pass (45 + ~2 for the updated tool count = 47).

- [ ] **Step 5: Commit**

```bash
git add helix-claude-mcp/src/tools.rs helix-claude-mcp/src/serve.rs
git commit -m "$(cat <<'EOF'
feat(claude-mcp): helix_format_document and helix_run_command tools

Two new ToolKind variants (now 9 total) with appropriate input schemas
and HelixFormatDocumentArgs / HelixRunCommandArgs structs.

helix_format_document is the LSP-formatter convenience: optional path,
defaults to active buffer. Returns applied: true when the format was
kicked off.

helix_run_command is the generic typable-command primitive. Tool
description explicitly warns about its power — it can do anything a
user could type at the `:` prompt. Examples in the description show
typical usage (write, reload, open path).

The call_tool match is still exhaustive (Rust compiler verified).

Refs: docs/specs/2026-05-12-helix-claude-mcp-bridge-design.md (§7.3)
EOF
)"
```

---

## Task 7: Integration tests for the two new tools

**Files:**
- Modify: `helix-claude-mcp/tests/integration.rs`

- [ ] **Step 1: Add two integration tests**

Append to `helix-claude-mcp/tests/integration.rs`:

```rust
#[tokio::test]
async fn tools_call_format_document_against_fake_helix() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    let canned = r#"{"method":"format-document","result":{"applied":true}}"#.to_string() + "\n";
    let _sock = spawn_fake_helix_in(tmp.path(), canned).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut child = Command::new(binary_path())
        .arg("serve")
        .env("CLAUDE_PROJECT_DIR", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    for msg in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"helix_format_document","arguments":{}}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..6 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                assert!(!line.contains("\"isError\":true"), "got error: {}", line);
                assert!(line.contains("applied"), "missing applied field: {}", line);
                found = true;
                break;
            }
        }
    }
    assert!(found, "no response");
    drop(stdin);
    let _ = child.kill().await;
}

#[tokio::test]
async fn tools_call_run_command_against_fake_helix() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    let canned = r#"{"method":"run-command","result":{"message":"context snapshot written"}}"#.to_string() + "\n";
    let _sock = spawn_fake_helix_in(tmp.path(), canned).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut child = Command::new(binary_path())
        .arg("serve")
        .env("CLAUDE_PROJECT_DIR", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    for msg in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"helix_run_command","arguments":{"name":"write-context"}}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..6 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                assert!(!line.contains("\"isError\":true"), "got error: {}", line);
                assert!(line.contains("context snapshot written"), "missing message: {}", line);
                found = true;
                break;
            }
        }
    }
    assert!(found, "no response");
    drop(stdin);
    let _ = child.kill().await;
}

#[tokio::test]
async fn tools_list_includes_phase_6_tools() {
    let tmp = TempDir::new().unwrap();
    let helix = tmp.path().join(".helix");
    std::fs::create_dir(&helix).unwrap();
    std::fs::write(helix.join("context.json"), SAMPLE_SNAPSHOT).unwrap();

    let mut child = Command::new(binary_path())
        .arg("serve")
        .env("CLAUDE_PROJECT_DIR", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    for msg in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"0.1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    ] {
        stdin.write_all(msg.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut found = false;
    for _ in 0..6 {
        let mut line = String::new();
        if let Ok(n) = reader.read_line(&mut line).await {
            if n == 0 { break; }
            if line.contains("\"id\":2") {
                assert!(line.contains("helix_format_document"), "missing format-document tool: {}", line);
                assert!(line.contains("helix_run_command"), "missing run-command tool: {}", line);
                found = true;
                break;
            }
        }
    }
    assert!(found, "no tools/list response");
    drop(stdin);
    let _ = child.kill().await;
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test -p helix-claude-mcp --test integration`
Expected: 6 prior + 3 new = 9 tests pass.

- [ ] **Step 3: Commit**

```bash
git add helix-claude-mcp/tests/integration.rs
git commit -m "$(cat <<'EOF'
test(claude-mcp): integration tests for helix_format_document and helix_run_command

Three new tests against fake-Helix listeners:
1. tools_call_format_document_against_fake_helix — call with empty args,
   fake responds with applied:true, assert response contains "applied"
2. tools_call_run_command_against_fake_helix — call with name:"write-context",
   fake responds with message:"context snapshot written", assert message
3. tools_list_includes_phase_6_tools — verifies the two new tools appear
   in the tools/list response

All tests use the same fake-Helix-listener helper from earlier Phase 4b
tests.
EOF
)"
```

---

## Task 8: README + final end-to-end smoke

**Files:**
- Modify: `helix-claude-mcp/README.md`

- [ ] **Step 1: Extend the README's tools table**

Edit `helix-claude-mcp/README.md`. Find the existing "Available Tools" table from Phase 4b. Add two new rows:

```markdown
| `helix_format_document` | Format a buffer using its LSP formatter. |
| `helix_run_command` | Execute any Helix typable command. **Powerful** — can write files, reload config, run shell commands. Use with care. |
```

- [ ] **Step 2: Final end-to-end smoke test**

Run the same all-methods test from Task 5 Step 5, but driven through the MCP bridge this time (to verify the full pipeline: Claude → MCP → helix-claude-mcp → JSON-RPC → Helix):

```bash
mkdir -p /tmp/p6-final && cd /tmp/p6-final && git init -q
cat > Cargo.toml <<'EOF'
[package]
name = "p6-final"
version = "0.1.0"
edition = "2021"
[[bin]]
name = "main"
path = "main.rs"
EOF
cat > main.rs <<'EOF'
fn   helper()->u32{42}
fn main(){let x=helper();println!("{}",x);}
EOF

# Build fresh
cd /Users/angm/helix
cargo build --release -p helix-term --bin hx 2>&1 | tail -2
cargo build --release -p helix-claude-mcp 2>&1 | tail -2

# Start Helix
cd /tmp/p6-final
script -q /dev/null /Users/angm/helix/target/release/hx main.rs &
HX_PID=$!
sleep 7

# Drive helix-claude-mcp through all 9 MCP tools (Phase 4b + Phase 6)
CLAUDE_PROJECT_DIR=/tmp/p6-final /Users/angm/helix/target/release/helix-claude-mcp serve <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"final","version":"0.1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"helix_get_diagnostics","arguments":{}}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"helix_get_workspace_symbols","arguments":{"query":"helper"}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"helix_get_hover","arguments":{"line":2,"column":14}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"helix_format_document","arguments":{}}}
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"helix_run_command","arguments":{"name":"write-context"}}}
{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"helix_open_file","arguments":{"path":"main.rs"}}}
EOF

# Verify the formatted file
sleep 2
cat /tmp/p6-final/main.rs

pkill -P $HX_PID; kill $HX_PID
sleep 1
rm -rf /tmp/p6-final
```

Expected:
- `tools/list` shows 9 tools including `helix_format_document` and `helix_run_command`
- `helix_get_diagnostics` returns empty list (or any structured response)
- `helix_get_workspace_symbols query=helper` finds the function
- `helix_get_hover` returns type info
- `helix_format_document` returns `{"applied":true}` content
- `helix_run_command name=write-context` returns the "context snapshot written" message
- `helix_open_file` returns ok
- After the sleep, main.rs is formatted (proper spacing, indentation by rustfmt)

If you can run this and all 8 method calls succeed, Phase 6 is verified end-to-end.

- [ ] **Step 3: Commit**

```bash
git add helix-claude-mcp/README.md
git commit -m "$(cat <<'EOF'
docs(claude-mcp): README lists Phase 6 tools

Extends the Available Tools table with helix_format_document and
helix_run_command. The latter has an explicit "powerful — use with
care" note.

Final end-to-end smoke covers all 9 MCP tools against a real Helix
+ rust-analyzer. Every tool returns expected output; the formatted
main.rs confirms LSP-formatting end-to-end through the bridge.

Phase 6 complete. The spec's Phase 1-6 are all shipped:
- Phase 1: context_logger + schema crate
- Phase 2a/b/c: control socket + 11 JSON-RPC methods
- Phase 3: LSP-backed methods (hover, definition, references, etc.)
- Phase 4a/b: helix-claude-mcp binary with 3 Resources + 9 Tools
- Phase 5: Rust hook subcommand
- Phase 6: format-document, run-command, :write-context

The full Helix ↔ Claude Code bridge is live.
EOF
)"
```

---

## Self-review checklist

After all 8 tasks:

- [ ] `cargo check --workspace` clean
- [ ] `cargo test -p helix-context-schema` — 51 tests pass (44 prior + 7 new in Task 1)
- [ ] `cargo test -p helix-term control_socket` — 15 still pass (the test rename in Task 5 doesn't change count)
- [ ] `cargo test -p helix-claude-mcp` — 48 pass (45 prior + 3 new integration in Task 7)
- [ ] `cargo build --release -p helix-term --bin hx` succeeds
- [ ] `cargo build --release -p helix-claude-mcp` succeeds
- [ ] Final smoke test in Task 8 Step 2 ran with real Helix + rust-analyzer; all 8 method calls returned non-error responses; main.rs is formatted after the bridge run
- [ ] `git log --oneline -10` shows the 8 Phase 6 commits in clean order

## What's NOT in Phase 6

- `helix-claude-mcp doctor` self-diagnostic subcommand
- `--verbose` flag for the hook + telemetry on `HookDecision::Skip` reasons
- `session_id` sanitization in the hook's marker path
- Cleanup of `dead_code` warnings on forward-looking `HookInput` fields
- Additional fake-Helix integration tests for the LSP read methods (hover, definition, references, workspace-symbols, diagnostics)

These are all minor polish items deferred to a Phase 6b round if the user wants them. The spec's Phase 1-6 deliverables are all complete with this plan.

## Open questions

1. **`Args::parse` signature.** Helix's Args type varies between branches. The plan's code assumes a specific signature (`parse(input, signature, validate, expand_callback)`). The actual signature in this fork may be different — implementer adapts in Tasks 3 and 4.

2. **`editor.get_status()` getter name.** Used to capture the status-line message for run-command's response. If no convenient getter exists, return `message: None` — not a blocker for correctness.

3. **format-document's async edits.** The plan returns `applied: true` as soon as the format command is kicked off; LSP edits arrive asynchronously. A future polish could `await` the format future before responding, but the current behavior matches what interactive `:format` does for the user.

4. **`run-command` security surface.** `helix_run_command` can execute anything a user could type at `:` — `:run-shell-command`, `:write-quit-all`, etc. The tool description explicitly warns Claude about this. If you want a hardened version, add an allow-list in `RunCommand` handler (limit to specific safe command names). Not part of Phase 6.
