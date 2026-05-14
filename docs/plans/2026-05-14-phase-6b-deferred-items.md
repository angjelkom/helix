# Phase 6b — Deferred items from the post-Phase-6 audit

> **For agentic workers:** Execute task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the seven items recorded in `docs/specs/2026-05-12-helix-mcp-bridge-design.md` §10b — small hardening passes that the Phase 6 ship explicitly deferred. Each task is independently shippable; tasks are ordered by real impact descending.

**Architecture:** No architectural change. All work happens inside the existing crates (`helix-mcp`, `helix-context-schema`, `helix-term/src/control_socket/`, `helix-term/src/application.rs`). No new crates, no new wire methods (except where noted in Task 5).

**Tech Stack:** Same as the rest of the bridge — Rust 1.95, Tokio, rmcp 1.6, serde, anyhow.

---

## Pre-flight: validation summary

Before drafting this plan I read each cited code path and reproduced the bugs that could be reproduced. The findings that survived scrutiny:

| Item | Reproduced? | Severity | Plan task |
|---|---|---|---|
| Snapshot text breaks `<helix-editor-context>` fence | **Yes** — a selection containing `</helix-editor-context>` punches through the fence and the trailing text reads as outside the block | HIGH | Task 1 |
| No overall RPC timeout — bridge can hang forever on a stuck Helix | **Confirmed in source** (rpc_client.rs has no `tokio::time::timeout`) | MEDIUM | Task 2 |
| Hook lacks `--verbose` for diagnosing skip reasons | **Confirmed** (decide() logs only at debug, no per-run breadcrumbs) | LOW | Task 3 |
| `helix-mcp doctor` self-diagnosis subcommand | **Confirmed missing** | MEDIUM | Task 4 |
| Bridge never sends `initialize` to Helix | **Confirmed** (Initialize only constructed in `#[cfg(test)]`) | LOW (forward-compat only) | Task 5 |
| `helix_run_command` has no allowlist/denylist | **Confirmed** — every TYPABLE_COMMAND is reachable | MEDIUM | Task 6 |
| `helix_open_file` accepts unconfined paths | **Confirmed** — absolute paths bypass workspace.join | LOW (opt-in) | Task 7 |

False-positive checks performed:

- "Initialize handshake matters at v2" — true but v2 doesn't exist; this is forward-compat only. Kept as a low-priority task; do not promote to higher.
- "Workspace-confinement on helix_open_file" — could break legitimate cross-workspace opens. Task 7 ships an **opt-in** flag, not a default-deny.
- "helix_run_command needs an env-var gate" — env-var gating adds shell-init friction. Task 6 ships a denylist of catastrophic commands instead, which is invisible to legitimate use.
- "Fence escape needs a per-emission nonce tag" — too invasive for the protection delivered. Task 1 ships content-scrubbing (refuse to emit when the closing tag appears in the body) which is minimal-impact and equally effective against accidental triggers.

---

## File Structure

Tasks edit existing files. No new files except the new `helix-mcp/src/doctor.rs` module added in Task 4.

```
helix-mcp/src/
├── main.rs        # Task 3 (--verbose), Task 4 (Doctor subcommand)
├── serve.rs       # Task 5 (initialize call), Task 6 (run-command denylist)
├── hook.rs        # Task 1 (fence-escape scrub), Task 3 (verbose telemetry)
├── rpc_client.rs  # Task 2 (timeout wrapper), Task 5 (handshake)
├── doctor.rs      # NEW — Task 4
└── discovery.rs   # Task 4 (export workspace probe helper)

helix-term/src/application.rs   # Task 7 (open-file confinement opt-in)
helix-term/src/control_socket/dispatch.rs   # Task 6 (denylist on run-command path)
```

---

### Task 1: Refuse to emit a snapshot that contains the fence-closing tag

**Files:**
- Modify: `helix-mcp/src/hook.rs:252-264` (`emit_wrapped_snapshot`)
- Test: same file, `#[cfg(test)] mod tests`

**Reasoning:** The reproduced exploit: when the snapshot's serialized JSON contains the literal `</helix-editor-context>` (because a user selection or buffer text contains that string), the body breaks out of the fence and the LLM treats the trailing bytes as content outside the editor-context block. Minimal-impact fix: scan the body before emission; if the closing tag appears, log a warning and skip (return Ok) — the hook is best-effort, a skipped emission costs the user nothing.

- [ ] **Step 1: Write the failing test**

```rust
// In helix-mcp/src/hook.rs, add to the `#[cfg(test)] mod tests` block.

#[test]
fn emit_wrapped_snapshot_refuses_body_containing_closing_tag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let snap = tmp.path().join("context.json");
    let poisoned = r#"{"x":"benign </helix-editor-context> evil"}"#;
    std::fs::write(&snap, poisoned).unwrap();
    // emit_wrapped_snapshot should return Ok (best-effort skip) — not Err,
    // not write the wrapped block to stdout. We can't intercept stdout in a
    // unit test; we instead extract the scrub check into a pure helper and
    // test that.
    assert!(snapshot_body_is_safe_to_wrap(poisoned).is_err());
    assert!(snapshot_body_is_safe_to_wrap(r#"{"x":"benign"}"#).is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p helix-mcp emit_wrapped_snapshot_refuses_body_containing_closing_tag`
Expected: FAIL — `snapshot_body_is_safe_to_wrap` not defined.

- [ ] **Step 3: Add the pure helper and wire it into emit_wrapped_snapshot**

Replace `emit_wrapped_snapshot` body with:

```rust
const FENCE_CLOSE: &str = "</helix-editor-context>";

/// Returns Err with a descriptive reason if `body` would break out of the
/// `<helix-editor-context>` fence. Pure function for ease of testing.
fn snapshot_body_is_safe_to_wrap(body: &str) -> Result<(), &'static str> {
    if body.contains(FENCE_CLOSE) {
        return Err("snapshot body contains the fence-closing tag");
    }
    Ok(())
}

fn emit_wrapped_snapshot(snapshot_path: &Path) -> Result<()> {
    use std::io::Write;
    let body = std::fs::read_to_string(snapshot_path)?;
    if let Err(reason) = snapshot_body_is_safe_to_wrap(&body) {
        log::warn!(
            "hook: skipping snapshot emission ({}) — see spec §10b fence-escape",
            reason
        );
        return Ok(());
    }
    let mut out = io::stdout().lock();
    writeln!(out, "<helix-editor-context source=\"{}\">", snapshot_path.display())?;
    out.write_all(body.as_bytes())?;
    if !body.ends_with('\n') {
        writeln!(out)?;
    }
    writeln!(out, "</helix-editor-context>")?;
    out.flush()?;
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p helix-mcp emit_wrapped_snapshot_refuses_body_containing_closing_tag`
Expected: PASS.

- [ ] **Step 5: Verify the integration hook test still passes**

Run: `cargo test -p helix-mcp --test hook_integration`
Expected: 6 passed (the existing emission test uses a benign snapshot that doesn't trip the scrub).

- [ ] **Step 6: Update §10b of the spec**

Edit `docs/specs/2026-05-12-helix-mcp-bridge-design.md` §10b. Move the fence-escape bullet from "Known limitations" to a sentence under §10's risk register saying the hook refuses to emit snapshots whose body contains the closing tag, and the user-visible failure mode is a missed injection on the rare prompts that would trip it.

- [ ] **Step 7: Commit**

```bash
git add helix-mcp/src/hook.rs docs/specs/2026-05-12-helix-mcp-bridge-design.md
git commit -m "fix(claude-mcp): refuse to emit snapshot containing fence-closing tag

Prompt-injection vector: a selected file region containing the literal
\`</helix-editor-context>\` would break the hook's wrapper fence, with
trailing bytes landing outside the block where the LLM reads them as
non-context content. Reproduced with a crafted snapshot.

Add a pure helper \`snapshot_body_is_safe_to_wrap\` that scans for the
fence-close substring and refuses emission when found. The hook is
best-effort; the user-visible failure mode is a missed injection on
the rare prompts that would have tripped it, not a hard error."
```

---

### Task 2: 30-second timeout on bridge → Helix RPC

**Files:**
- Modify: `helix-mcp/src/rpc_client.rs:32-62` (`send_request`)
- Modify: same file, `RpcError` enum

**Reasoning:** `UnixStream::connect` has a 200ms connect timeout via discovery, but once connected, `write_all` and `read_line` have no deadlines. If Helix's event loop hangs (e.g., a Steel hook blocks the main thread, or LSP-future deadlock), the bridge call hangs with it. Claude Code's own per-tool timeout might catch it, but we should be defensive at our layer.

- [ ] **Step 1: Write the failing test**

```rust
// In helix-mcp/src/rpc_client.rs `#[cfg(test)] mod tests`.

#[tokio::test]
async fn send_request_times_out_when_helix_never_responds() {
    use tokio::time::Duration;
    let tmp = tempfile::TempDir::new().unwrap();
    let sock = tmp.path().join("hanging.sock");
    let _listener = UnixListener::bind(&sock).unwrap();
    // Accept the connection but never write anything.
    let accept_task = tokio::spawn(async move {
        let _ = _listener.accept().await;
        // Hold the stream open without writing.
        tokio::time::sleep(Duration::from_secs(60)).await;
    });

    let req = ControlRequest::Initialize {
        protocol_version: "1.0".into(),
        client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
    };
    let start = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        send_request_with_timeout(&sock, &req, Duration::from_millis(500)),
    )
    .await
    .expect("outer timeout (test framework safeguard)");
    assert!(matches!(result, Err(RpcError::Timeout(_))), "got: {:?}", result);
    assert!(start.elapsed() < Duration::from_secs(2));
    accept_task.abort();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p helix-mcp send_request_times_out_when_helix_never_responds`
Expected: FAIL — `send_request_with_timeout` not defined, `RpcError::Timeout` not defined.

- [ ] **Step 3: Add timeout variant and a `send_request_with_timeout` helper**

```rust
// In rpc_client.rs RpcError enum, add:
#[error("timed out after {0:?} waiting for Helix to respond")]
Timeout(std::time::Duration),

// Then add this helper alongside send_request:
const DEFAULT_RPC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub async fn send_request_with_timeout(
    socket_path: &Path,
    request: &ControlRequest,
    timeout: std::time::Duration,
) -> Result<ControlResponse, RpcError> {
    match tokio::time::timeout(timeout, send_request(socket_path, request)).await {
        Ok(r) => r,
        Err(_) => Err(RpcError::Timeout(timeout)),
    }
}
```

- [ ] **Step 4: Make `serve.rs::dispatch_tool` use the wrapped version**

Change the line in `helix-mcp/src/serve.rs` that calls `rpc_client::send_request(&socket, &request)` to `rpc_client::send_request_with_timeout(&socket, &request, DEFAULT_RPC_TIMEOUT)`, importing `DEFAULT_RPC_TIMEOUT` from `crate::rpc_client`.

- [ ] **Step 5: Run all tests**

Run: `cargo test -p helix-mcp`
Expected: PASS — new timeout test passes, no regressions in the 39 + 6 + 15 existing tests.

- [ ] **Step 6: Commit**

```bash
git add helix-mcp/src/rpc_client.rs helix-mcp/src/serve.rs
git commit -m "fix(claude-mcp): bound bridge → Helix RPC at 30s

A stuck Helix event loop (Steel hook blocking main thread, deadlocked
LSP future) would hang the MCP tool call indefinitely. Discovery has
its own 200ms connect timeout but the write/read phase had no deadline.

Add \`send_request_with_timeout\` and a 30s default; dispatch_tool
uses it for every tool call. The wrapper returns \`RpcError::Timeout\`
which surfaces to Claude as a structured tool error."
```

---

### Task 3: `--verbose` flag on `hook` subcommand for diagnostic breadcrumbs

**Files:**
- Modify: `helix-mcp/src/main.rs:34-46` (Hook variant of Command enum)
- Modify: `helix-mcp/src/hook.rs:207-249` (`run` function)

**Reasoning:** Today, debugging why the hook skipped requires `RUST_LOG=debug helix-mcp hook`, which is unergonomic when the hook is wired via settings.json (you'd need to add an env var to every entry). A `--verbose` flag that logs each decision step on stderr is a small UX win.

- [ ] **Step 1: Write the failing integration test**

```rust
// In helix-mcp/tests/hook_integration.rs:

#[tokio::test]
async fn verbose_flag_emits_decision_on_stderr() {
    let workspace = TempDir::new().unwrap();
    write_minimal_snapshot(workspace.path(), "focus_lost");
    let output = run_hook_with_args(
        &workspace,
        "verbose-probe-001",
        &["--verbose"],
    ).await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("decision=") || stderr.contains("Emit"),
        "expected verbose decision log on stderr, got: {}",
        stderr
    );
    assert!(output.status.success());
}
```

(Reuse the existing test helpers in hook_integration.rs; add a `run_hook_with_args` variant if one doesn't exist.)

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — `--verbose` not recognized by clap.

- [ ] **Step 3: Add the flag to the Command enum**

```rust
// In helix-mcp/src/main.rs, replace the Hook variant with:
Hook {
    #[arg(long)]
    reset_marker: bool,
    /// Emit decision breadcrumbs on stderr (snapshot path, mtime,
    /// marker state, final decision). Diagnostic — does not affect
    /// emission behavior. When unset, only warn/error logs appear.
    #[arg(long)]
    verbose: bool,
},
```

Update the match arm in main to call `hook::run(reset_marker, verbose).await`.

- [ ] **Step 4: Thread the flag through `hook::run`**

In `helix-mcp/src/hook.rs`:

```rust
pub async fn run(reset_marker: bool, verbose: bool) -> Result<()> {
    // ... existing stdin parse, reset_marker branch unchanged ...

    let decision = decide(&input);
    if verbose {
        eprintln!("helix-mcp hook: decision={:?}", decision);
    }
    match decision {
        HookDecision::Skip(reason) => {
            log::debug!("hook: skip ({})", reason);
            Ok(())
        }
        HookDecision::Emit { snapshot_path, snapshot_mtime } => {
            if let Err(e) = emit_wrapped_snapshot(&snapshot_path) {
                if verbose {
                    eprintln!("helix-mcp hook: emit failed: {}", e);
                }
                log::warn!("hook: emitting snapshot failed: {}", e);
                return Ok(());
            }
            let marker_p = marker_path(&input.session_id);
            if let Err(e) = write_marker_mtime(&marker_p, snapshot_mtime) {
                if verbose {
                    eprintln!("helix-mcp hook: marker write failed: {}", e);
                }
                log::warn!("hook: writing marker failed: {}", e);
            }
            Ok(())
        }
    }
}
```

- [ ] **Step 5: Run all tests**

Run: `cargo test -p helix-mcp`
Expected: PASS. New `--verbose` test green; existing tests unaffected (the flag defaults to `false`).

- [ ] **Step 6: Update README**

In `helix-mcp/README.md`, add a one-paragraph note about `--verbose` in the hook subcommand section.

- [ ] **Step 7: Commit**

```bash
git add helix-mcp/src/main.rs helix-mcp/src/hook.rs helix-mcp/tests/hook_integration.rs helix-mcp/README.md
git commit -m "feat(claude-mcp): --verbose flag on hook subcommand

Adds a diagnostic --verbose flag that emits the hook's decision and
any emit/marker-write failures to stderr. Useful for debugging \"why
didn't my snapshot inject\" without setting RUST_LOG=debug on every
hook entry in settings.json.

Default-off; existing behavior unchanged."
```

---

### Task 4: `helix-mcp doctor` self-diagnosis subcommand

**Files:**
- Create: `helix-mcp/src/doctor.rs`
- Modify: `helix-mcp/src/main.rs` (Command enum + dispatch)
- Modify: `helix-mcp/src/discovery.rs` (export `is_socket_live` if currently private)

**Reasoning:** New users with broken setups don't know which piece failed (binary on PATH, snapshot file present, socket connectable, initialize succeeds, schema matches). `doctor` runs all five checks and prints a human-readable report.

- [ ] **Step 1: Write the failing unit test for the report formatter**

```rust
// In helix-mcp/src/doctor.rs (new file). Start with:
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_renders_all_checks() {
        let r = Report {
            binary_on_path: Some("/Users/test/.cargo/bin/helix-mcp".into()),
            workspace: Some(std::path::PathBuf::from("/tmp/ws")),
            snapshot: SnapshotCheck::Found {
                path: "/tmp/ws/.helix/context.json".into(),
                schema_version: 2,
                age_secs: 12,
            },
            socket: SocketCheck::Live("/tmp/ws/.helix/control-123.sock".into()),
            initialize: InitializeCheck::Ok("1.0".into()),
        };
        let s = r.render();
        assert!(s.contains("binary"));
        assert!(s.contains("snapshot"));
        assert!(s.contains("socket"));
        assert!(s.contains("initialize"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — `doctor.rs` doesn't exist yet.

- [ ] **Step 3: Implement the doctor module**

Create `helix-mcp/src/doctor.rs`:

```rust
//! `doctor` subcommand. Five checks, one report.

use std::path::{Path, PathBuf};

use crate::{discovery, resources, rpc_client};
use helix_context_schema::{ClientInfo, ContextSnapshot, ControlRequest};

pub struct Report {
    pub binary_on_path: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub snapshot: SnapshotCheck,
    pub socket: SocketCheck,
    pub initialize: InitializeCheck,
}

pub enum SnapshotCheck {
    Missing(PathBuf),
    Unreadable(PathBuf, String),
    InvalidJson(PathBuf, String),
    Found {
        path: PathBuf,
        schema_version: u32,
        age_secs: u64,
    },
}

pub enum SocketCheck {
    None,
    Stale(PathBuf),
    Live(PathBuf),
}

pub enum InitializeCheck {
    Skipped,
    Ok(String),
    Failed(String),
}

impl Report {
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str("helix-mcp doctor\n================\n\n");
        s.push_str(&format!(
            "binary on PATH      : {}\n",
            self.binary_on_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(NOT FOUND — install with `cargo install --path helix-mcp`)".into())
        ));
        s.push_str(&format!(
            "workspace           : {}\n",
            self.workspace
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(unable to resolve — CLAUDE_PROJECT_DIR unset and cwd has no .helix/ ancestor)".into())
        ));
        s.push_str("snapshot            : ");
        match &self.snapshot {
            SnapshotCheck::Missing(p) => s.push_str(&format!("MISSING at {}\n", p.display())),
            SnapshotCheck::Unreadable(p, e) => s.push_str(&format!("UNREADABLE at {}: {}\n", p.display(), e)),
            SnapshotCheck::InvalidJson(p, e) => s.push_str(&format!("INVALID JSON at {}: {}\n", p.display(), e)),
            SnapshotCheck::Found { path, schema_version, age_secs } => s.push_str(&format!(
                "OK at {} (schema_version={}, {}s old)\n",
                path.display(),
                schema_version,
                age_secs
            )),
        }
        s.push_str("control socket      : ");
        match &self.socket {
            SocketCheck::None => s.push_str("NOT FOUND (Helix isn't running, or [editor.control-socket] enabled = false)\n"),
            SocketCheck::Stale(p) => s.push_str(&format!("STALE at {} (no listener)\n", p.display())),
            SocketCheck::Live(p) => s.push_str(&format!("LIVE at {}\n", p.display())),
        }
        s.push_str("initialize handshake: ");
        match &self.initialize {
            InitializeCheck::Skipped => s.push_str("(skipped — no live socket)\n"),
            InitializeCheck::Ok(v) => s.push_str(&format!("OK (helix protocol_version={})\n", v)),
            InitializeCheck::Failed(e) => s.push_str(&format!("FAILED: {}\n", e)),
        }
        s
    }
}

pub async fn run() -> Result<(), anyhow::Error> {
    // ... assembled from the constituent checks; see Step 4 ...
    let report = collect_report().await;
    print!("{}", report.render());
    Ok(())
}

async fn collect_report() -> Report {
    // ... in Step 4 ...
    unimplemented!()
}
```

- [ ] **Step 4: Run unit test to verify it passes**

Run: `cargo test -p helix-mcp doctor::tests::report_renders_all_checks`
Expected: PASS.

- [ ] **Step 5: Implement `collect_report` against the existing modules**

Replace the `unimplemented!` body with:

```rust
async fn collect_report() -> Report {
    let binary_on_path = which::which("helix-mcp").ok();
    let workspace = resources::resolve_workspace(None).ok();

    let snapshot = match &workspace {
        Some(ws) => check_snapshot(ws),
        None => SnapshotCheck::Missing(PathBuf::from("(no workspace)")),
    };

    let (socket, initialize) = match discovery::find_helix_socket(workspace.as_deref()).await {
        Ok(sock) => {
            let init = probe_initialize(&sock).await;
            (SocketCheck::Live(sock), init)
        }
        Err(_) => (SocketCheck::None, InitializeCheck::Skipped),
    };

    Report { binary_on_path, workspace, snapshot, socket, initialize }
}

fn check_snapshot(workspace: &Path) -> SnapshotCheck {
    let path = workspace.join(".helix").join("context.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SnapshotCheck::Missing(path),
        Err(e) => return SnapshotCheck::Unreadable(path, e.to_string()),
    };
    let snap: ContextSnapshot = match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => return SnapshotCheck::InvalidJson(path, e.to_string()),
    };
    let age_secs = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    SnapshotCheck::Found {
        path,
        schema_version: snap.schema_version,
        age_secs,
    }
}

async fn probe_initialize(socket: &Path) -> InitializeCheck {
    let req = ControlRequest::Initialize {
        protocol_version: helix_context_schema::PROTOCOL_VERSION.into(),
        client_info: ClientInfo {
            name: "helix-mcp doctor".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    };
    match rpc_client::send_request_with_timeout(
        socket,
        &req,
        std::time::Duration::from_secs(5),
    )
    .await
    {
        Ok(helix_context_schema::ControlResponse::Initialize {
            protocol_version, ..
        }) => InitializeCheck::Ok(protocol_version),
        Ok(_) => InitializeCheck::Failed("unexpected response shape".into()),
        Err(e) => InitializeCheck::Failed(e.to_string()),
    }
}
```

Note: this depends on Task 2's `send_request_with_timeout`. Ship Task 2 first or include the timeout wrapper inline here.

- [ ] **Step 6: Wire the subcommand into main.rs**

```rust
// In main.rs, add to Command enum:
/// Run a self-diagnosis: binary on PATH, snapshot present and
/// parseable, socket connectable, initialize handshake. Useful when
/// onboarding a new install or debugging a broken connection.
Doctor,

// In the match in main(), add:
Command::Doctor => crate::doctor::run().await,

// At the top of main.rs, add the module:
mod doctor;
```

Add the `which = "6"` dep to `helix-mcp/Cargo.toml` (no transitive baggage; ~5 KB).

- [ ] **Step 7: Manual smoke test**

Run: `helix-mcp doctor`
Expected: prints a five-line report; if Helix is running, shows LIVE socket and OK initialize.

- [ ] **Step 8: Commit**

```bash
git add helix-mcp/src/doctor.rs helix-mcp/src/main.rs helix-mcp/Cargo.toml helix-mcp/Cargo.lock
git commit -m "feat(claude-mcp): doctor self-diagnosis subcommand

Reports binary-on-PATH, resolved workspace, snapshot presence and
schema version, control-socket liveness, and initialize handshake.
Five checks, human-readable output. Used by new installs and when
debugging broken connections."
```

---

### Task 5: Send `initialize` handshake from bridge to Helix on first tool call

**Files:**
- Modify: `helix-mcp/src/serve.rs::dispatch_tool`
- Modify: `helix-mcp/src/rpc_client.rs` (add a one-shot handshake helper)
- New unit test in `serve.rs` or a small integration test

**Reasoning:** Spec §6.1 mandates the handshake; the bridge never sends it. Today both ends are v1 so the gap is invisible. A future v2 bump would surface confusing parse errors instead of a clean version-mismatch refusal. Cheap to add: one round-trip per process, cached for the life of `serve`.

**Important:** Per-process cache, not per-connection — the bridge opens a fresh UnixStream per tool call (no connection pooling). Caching the handshake result means we only do it once per `helix-mcp serve` lifetime.

- [ ] **Step 1: Write the failing test**

```rust
// In helix-mcp/src/serve.rs tests (or a new test file).

#[tokio::test]
async fn dispatch_tool_sends_initialize_before_first_tool_call() {
    // Spawn a fake Helix that records every method it receives.
    // Assert the first request is method=initialize.
    // (Use the same spawn_fake_helix pattern from tests/integration.rs, but
    // here we record received methods instead of canning a response.)
    // ...
    assert_eq!(received_methods.first(), Some(&"initialize".to_string()));
}
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — bridge sends the tool's method directly.

- [ ] **Step 3: Add handshake helper to rpc_client.rs**

```rust
use std::sync::OnceLock;

static HANDSHAKE_CACHE: OnceLock<tokio::sync::Mutex<Option<HandshakeOutcome>>> = OnceLock::new();

#[derive(Clone)]
pub enum HandshakeOutcome {
    Ok { helix_version: String, protocol_version: String },
    Incompatible(String),
}

/// Send `initialize` once per process. Subsequent calls return the cached
/// outcome. Returns `Err` if the version is incompatible — caller should
/// surface that to Claude rather than attempting the tool.
pub async fn ensure_handshake(socket_path: &Path) -> Result<HandshakeOutcome, RpcError> {
    let cache = HANDSHAKE_CACHE.get_or_init(|| tokio::sync::Mutex::new(None));
    let mut guard = cache.lock().await;
    if let Some(cached) = guard.as_ref() {
        return Ok(cached.clone());
    }
    let req = ControlRequest::Initialize {
        protocol_version: helix_context_schema::PROTOCOL_VERSION.into(),
        client_info: ClientInfo {
            name: env!("CARGO_PKG_NAME").into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    };
    match send_request_with_timeout(socket_path, &req, std::time::Duration::from_secs(5)).await? {
        ControlResponse::Initialize { protocol_version, helix_version, .. } => {
            let outcome = if is_compatible_major(&protocol_version, helix_context_schema::PROTOCOL_VERSION) {
                HandshakeOutcome::Ok { helix_version, protocol_version }
            } else {
                HandshakeOutcome::Incompatible(format!(
                    "helix protocol_version={} incompatible with bridge {}",
                    protocol_version,
                    helix_context_schema::PROTOCOL_VERSION
                ))
            };
            *guard = Some(outcome.clone());
            Ok(outcome)
        }
        _ => Err(RpcError::Parse(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "initialize returned wrong variant",
        )))),
    }
}

fn is_compatible_major(a: &str, b: &str) -> bool {
    let major = |s: &str| s.split('.').next().and_then(|n| n.parse::<u32>().ok());
    matches!((major(a), major(b)), (Some(x), Some(y)) if x == y)
}
```

- [ ] **Step 4: Call from `dispatch_tool`**

```rust
// At the top of dispatch_tool in serve.rs, after socket discovery:
match rpc_client::ensure_handshake(&socket).await {
    Ok(HandshakeOutcome::Ok { .. }) => {}
    Ok(HandshakeOutcome::Incompatible(msg)) => {
        return tool_error(format!(
            "Helix and the bridge speak incompatible protocol versions: {}. \
             Upgrade whichever is older.",
            msg
        ));
    }
    Err(e) => {
        return tool_error(format!(
            "Handshake with Helix failed: {}. Subsequent calls will retry.",
            e
        ));
    }
}
```

- [ ] **Step 5: Run all tests**

Run: `cargo test -p helix-mcp`
Expected: PASS — including the new handshake test. Existing fake-Helix tests need updating: they currently respond to *any* request with the canned response, so they accidentally pass — but after this change, the bridge sends Initialize first, which the fake responds to with the wrong shape. Update `spawn_fake_helix_in` to respond appropriately to Initialize on the first connect.

- [ ] **Step 6: Commit**

```bash
git add helix-mcp/src/rpc_client.rs helix-mcp/src/serve.rs helix-mcp/tests/integration.rs
git commit -m "feat(claude-mcp): send initialize handshake before first tool call

Spec §6.1 mandates the handshake but the bridge never sent it. Today
the protocol is at v1 on both sides so the gap is invisible. A future
v2 bump would surface as a confusing parse failure instead of a clean
version-mismatch refusal.

Cache the handshake per process (one round-trip for the lifetime of
\`helix-mcp serve\`, regardless of how many tool calls follow)."
```

---

### Task 6: Denylist of catastrophic typable commands on `helix_run_command`

**Files:**
- Modify: `helix-term/src/application.rs` (RunCommand handler around line 2477)
- Modify: `helix-mcp/src/tools.rs` (description text update)

**Reasoning:** `helix_run_command` is documented as "POWERFUL". A prompt-injected Claude call could do `:run-shell-command rm -rf ~` or `:quit!` and destroy unsaved work. Minimal-impact mitigation: block the specific commands whose damage cannot be undone via normal editing, on the Helix side (so even if the bridge is bypassed or another client connects, the protection holds).

**Design choice — deny these:**

| Command | Reason |
|---|---|
| `quit!` / `q!` / `qa!` / `quit-all!` | Discards unsaved work; user can't recover. |
| `run-shell-command` / `sh` / `bang` / `!` | Arbitrary shell exec via the editor. The user has terminal access anyway; routing through Helix adds no capability but does add an injection surface. |
| `pipe` / `pipe-to` | Arbitrary shell exec via pipe through buffer. Same reasoning. |

**Allow:** everything else, including `:write`, `:reload`, `:format`, `:theme`, `:set`, etc. The user is the threat model; we're protecting them from accidental Claude actions, not from themselves.

- [ ] **Step 1: Write the failing test**

```rust
// In helix-term/src/application.rs (new #[cfg(test)] mod for the dispatch path),
// or — easier — add as an integration test in helix-mcp/tests/ that exercises
// the deny path via a real Helix run (which is heavy). The cheapest option:
// extract the denylist check into a pure helper and unit-test that.

// In application.rs, add near the RunCommand handler:
fn is_destructive_typable_command(name: &str) -> bool {
    matches!(
        name,
        "quit!" | "q!" | "quit-all!" | "qa!"
        | "run-shell-command" | "sh" | "bang" | "!"
        | "pipe" | "pipe-to"
    )
}

#[cfg(test)]
mod run_command_denylist_tests {
    use super::is_destructive_typable_command;

    #[test]
    fn denies_force_quits() {
        for name in ["quit!", "q!", "quit-all!", "qa!"] {
            assert!(is_destructive_typable_command(name), "should deny {}", name);
        }
    }

    #[test]
    fn denies_shell_execs() {
        for name in ["run-shell-command", "sh", "bang", "!", "pipe", "pipe-to"] {
            assert!(is_destructive_typable_command(name), "should deny {}", name);
        }
    }

    #[test]
    fn allows_common_safe_commands() {
        for name in ["write", "w", "reload", "format", "theme", "set"] {
            assert!(!is_destructive_typable_command(name), "should allow {}", name);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — function not yet defined.

- [ ] **Step 3: Add the helper and use it**

In `helix-term/src/application.rs`, inside the `ControlRequest::RunCommand` arm, before `TYPABLE_COMMAND_MAP.get(...)`, add:

```rust
if is_destructive_typable_command(&name) {
    let _ = reply.send(Err(JsonRpcError {
        code: JsonRpcErrorCode::InvalidParams,
        message: format!(
            "command '{}' is denied by helix_run_command for safety. \
             To opt in, set $HELIX_CONTROL_SOCKET_ALLOW_DESTRUCTIVE=1 \
             before starting Helix.",
            name
        ),
        data: None,
    }));
    return;
}
```

Wrap with the env-var opt-out:

```rust
fn is_destructive_typable_command(name: &str) -> bool {
    if std::env::var_os("HELIX_CONTROL_SOCKET_ALLOW_DESTRUCTIVE").is_some() {
        return false;
    }
    matches!(/* ... */)
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test -p helix-term run_command_denylist`
Expected: PASS for the three new tests.

Run: `cargo test -p helix-term --lib control_socket::`
Expected: PASS — the 16 existing control_socket tests don't exercise the denied commands so should be unaffected.

- [ ] **Step 5: Update the MCP tool description**

In `helix-mcp/src/tools.rs::description`, replace the `HelixRunCommand` arm:

```rust
Self::HelixRunCommand => {
    "Execute a Helix typable command. Useful for { name: 'write' } to save, \
     { name: 'reload' } to reload from disk, { name: 'format' } to format \
     via the LSP, etc. By default, a small denylist refuses commands that \
     destroy unsaved work without recovery (`quit!`, `q!`, `quit-all!`, \
     `qa!`) and commands that exec arbitrary shell (`run-shell-command`, \
     `sh`, `bang`, `!`, `pipe`, `pipe-to`). To override, set \
     `HELIX_CONTROL_SOCKET_ALLOW_DESTRUCTIVE=1` before starting Helix. \
     Pass `name` without the leading colon; pass `args` for additional \
     positionals (each element becomes one token, no shell parsing)."
}
```

Also update the `SERVER_INSTRUCTIONS` `helix_run_command` line in `serve.rs` to mention the denylist.

- [ ] **Step 6: Update spec §10b**

In `docs/specs/2026-05-12-helix-mcp-bridge-design.md`, edit §10b's `helix_run_command` bullet to describe the shipped denylist and the env-var opt-out.

- [ ] **Step 7: Commit**

```bash
git add helix-term/src/application.rs helix-mcp/src/tools.rs helix-mcp/src/serve.rs docs/specs/2026-05-12-helix-mcp-bridge-design.md
git commit -m "feat(control-socket): denylist destructive commands on helix_run_command

A prompt-injected MCP call to helix_run_command could discard unsaved
work (\`:quit!\`) or exec arbitrary shell (\`:run-shell-command rm
-rf\`). Add a small denylist (force-quits + shell-execs) on the Helix
side so the protection holds regardless of which client connects.

Opt-out via HELIX_CONTROL_SOCKET_ALLOW_DESTRUCTIVE=1 for users who
deliberately want unrestricted access."
```

---

### Task 7: Opt-in workspace-confinement for `helix_open_file`

**Files:**
- Modify: `helix-term/src/application.rs::ControlRequest::OpenFile` handler (around line 1928)
- Modify: `helix-context-schema/src/types.rs::ContextLoggerConfig` or add a new config block

**Reasoning:** Today `helix_open_file` accepts any path the user can read. That's deliberate — `helix_open_file('/etc/hosts')` is sometimes useful. But on shared workstations or paranoid setups, a user may want to confine opens to the workspace tree. Ship an opt-in config flag rather than a default-deny.

- [ ] **Step 1: Write the failing test for the prefix-check helper**

```rust
// In helix-term/src/application.rs:

fn is_path_within_workspace(path: &Path, workspace: &Path) -> bool {
    match (path.canonicalize(), workspace.canonicalize()) {
        (Ok(p), Ok(ws)) => p.starts_with(&ws),
        _ => false,
    }
}

#[cfg(test)]
mod open_file_confinement_tests {
    use super::is_path_within_workspace;
    use tempfile::TempDir;

    #[test]
    fn rejects_absolute_path_outside_workspace() {
        let ws = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("a.txt");
        std::fs::write(&outside_file, "").unwrap();
        assert!(!is_path_within_workspace(&outside_file, ws.path()));
    }

    #[test]
    fn accepts_path_inside_workspace() {
        let ws = TempDir::new().unwrap();
        let inside = ws.path().join("a.txt");
        std::fs::write(&inside, "").unwrap();
        assert!(is_path_within_workspace(&inside, ws.path()));
    }
}
```

- [ ] **Step 2: Add the config flag**

In `helix-view/src/editor.rs::ControlSocketConfig` (or wherever the control-socket config struct lives), add:

```rust
/// When true, helix_open_file refuses to open files outside the current
/// workspace tree (after canonicalization). Default false — the user may
/// legitimately want to open arbitrary files via Claude.
#[serde(default)]
pub confine_open_file_to_workspace: bool,
```

- [ ] **Step 3: Apply the check in the handler**

In the `ControlRequest::OpenFile` arm of `application.rs`, after computing `resolved_path` and before `editor.open`:

```rust
if self.editor.config().control_socket.confine_open_file_to_workspace
    && !is_cwd_fallback
    && !is_path_within_workspace(&resolved_path, &workspace)
{
    let _ = reply.send(Err(JsonRpcError {
        code: JsonRpcErrorCode::PathOutsideWorkspace,
        message: format!(
            "path {} is outside the workspace {} (confine_open_file_to_workspace = true)",
            resolved_path.display(),
            workspace.display()
        ),
        data: None,
    }));
    return;
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p helix-term open_file_confinement`
Expected: PASS — 2 new tests.

Run: `cargo test -p helix-term --lib control_socket::`
Expected: PASS — existing tests don't enable `confine_open_file_to_workspace` so behavior is unchanged.

- [ ] **Step 5: Document in README + spec**

Add a one-line note to the `[editor.control-socket]` section of `README.md`:

```toml
[editor.control-socket]
enabled = true
confine-open-file-to-workspace = false   # opt-in: refuse helix_open_file paths outside the workspace tree
```

Add a paragraph to spec §5.1 describing the flag.

- [ ] **Step 6: Commit**

```bash
git add helix-term/src/application.rs helix-view/src/editor.rs README.md docs/specs/2026-05-12-helix-mcp-bridge-design.md
git commit -m "feat(control-socket): opt-in workspace-confinement on helix_open_file

\`[editor.control-socket].confine-open-file-to-workspace\` (default
false) refuses helix_open_file paths that don't canonicalize under the
workspace tree. Opt-in because cross-workspace opens are sometimes
legitimate (system headers, sibling repos)."
```

---

## Order of execution

Independent: Tasks 1, 2, 3, 4, 7 can land in any order. Task 5 depends on Task 2's `send_request_with_timeout`; ship Task 2 first. Task 6 is independent. Suggested order if shipping individually:

1. Task 1 (fence escape) — only active concern
2. Task 2 (RPC timeout) — defensive baseline
3. Task 3 (verbose flag) — small UX win
4. Task 4 (doctor) — improves onboarding
5. Task 6 (denylist) — design choice; review before merge
6. Task 5 (handshake) — forward-compat only
7. Task 7 (open-file confinement) — opt-in, defer until requested

---

## Self-doubt and criticism

Things in this plan I'm uncertain about. A reviewer should challenge these before merging:

1. **Task 1's scrub is a refusal, not a fix.** When the snapshot contains the closing tag, the hook silently skips emission. The user gets no editor context for that prompt and may not realize why. Alternative: replace `</helix-editor-context>` in the body with `</helix-editor-context-escaped>` (or `<\/helix-editor-context>`, which JSON tolerates) so emission proceeds with a slightly munged body. I judged "skip is honest, mangling is misleading" — but a reviewer could reasonably disagree.

2. **Task 2's 30s default is a guess.** I picked 30s because it's well past any LSP timeout (10s) and Helix wait_event tick rates but short enough to feel like a real timeout. There's no measurement behind it. Worth A/B'ing if anyone uses the bridge under load.

3. **Task 5's handshake cache is per-process, not per-connection.** That assumes Helix won't restart with a different protocol version during a single `helix-mcp serve` lifetime. True today (the bridge respawns per Claude Code session) but fragile if anyone runs `serve` as a long-lived daemon. Acceptable for v1.

4. **Task 6's denylist is opinionated.** I excluded `:write` from the deny list because saving a file is recoverable — but `:write` to an unintended path could overwrite a file. I excluded `:reload` for the same reason. A more paranoid reviewer might want a positive *allowlist* instead. I went with denylist because allowlists become churn over time (every new safe command needs a PR). This is a real design trade-off; mention it in code review.

5. **Task 6's env-var opt-out works, but is invisible.** If a user sets `HELIX_CONTROL_SOCKET_ALLOW_DESTRUCTIVE=1` once and forgets, every future Helix session has it. There's no way for the bridge or for Claude to know the user is opted in. The denial message hints at it, which is what saves a lost user.

6. **Task 7 is small but the surface area is doc-heavy.** The flag is one line of code and three of config plumbing, but it adds another knob to `[editor.control-socket]` users have to know about. If we're not careful, the spec grows config knobs faster than features. Worth questioning whether anyone will actually flip this.

7. **Task 4 (doctor) adds a `which` crate dependency.** Tiny (no transitive baggage), but a new dep nonetheless. An alternative is reading `$PATH` manually — saves the dep but is OS-specific. Probably not worth it.

8. **Two findings from the post-Phase-6 audit are NOT in this plan:**
   - "Hook propagates errors to non-zero exit" — fixed already in `846f0d4be`.
   - "Marker dir 0755 race window" — fixed already in `846f0d4be`.
   I checked the commit log to confirm before omitting. If a reviewer notices the gap, that's why.

9. **What I might be missing:** there's no task for "what happens when MCP `instructions` get out of sync with code reality." Today they're a single source of truth in `SERVER_INSTRUCTIONS`, but nothing enforces that all 10 tool names are mentioned, or that the navigate-before-edit recipe matches the current tool surface. A future regression that adds an 11th tool would silently leave the instructions wrong. Worth a CI check (parse `SERVER_INSTRUCTIONS`, look for every `ToolKind::all()` name) — but I'm not adding it here because the cost/benefit is marginal and the failure mode is graceful (an extra tool just isn't documented in the instructions).

10. **What this plan won't accomplish:** none of these tasks fundamentally change what the bridge is. They're hardening, not features. After all seven ship, the bridge is the same shape it is today — just sturdier against accidents and easier to debug when broken.
