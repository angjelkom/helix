# Helix ↔ Claude Code Bridge — Phase 7 Improvements

**Status:** Draft, post-Phase-6b
**Date:** 2026-05-14
**Author:** Angjelko (synthesizing audit reports from four parallel agents)
**Supersedes:** Builds on `docs/specs/2026-05-12-helix-mcp-bridge-design.md` (Phases 1–6) and `docs/plans/2026-05-14-phase-6b-deferred-items.md` (Phase 6b residual hardening, all shipped).

## 1. Overview

After Phase 6b shipped the residual-risk items from the post-Phase-6 audit, the bridge is functionally complete: 10 MCP tools, 3 resources, fence-escape mitigation, RPC timeout, initialize handshake with per-process cache, run-command denylist, doctor subcommand. This document gathers the next round of improvements identified by a fresh four-angle audit (performance, feature gaps, code quality, operational maturity) and proposes a Phase 7 work program.

Nothing here is required for the bridge to function. Everything here is an investment that pays back at scale — when the bridge is used by many developers, runs millions of hook invocations, or grows new tools fast enough that the current dispatch shape becomes a maintenance tax.

## 2. Audit methodology

Four agents were dispatched in parallel against the live `nightly` branch:

| Angle | What they looked for | Constraint |
|---|---|---|
| Performance | Hot-path syscall reduction, allocation patterns, hook efficiency, snapshot-write cost | Quantified wins only; no architecture changes |
| Feature gaps | Tools/resources the editor uniquely knows about that Claude doesn't | Must beat Claude's existing Read/Edit/Grep ergonomics |
| Code quality | Refactors that improve maintainability or correctness without changing behavior | ≥ 50 LOC saved or load-bearing readability gain |
| Operational maturity | Observability, multi-instance, deployment hygiene, failure recovery | Concrete user-visible scenario required |

Each agent was instructed to cite specific code with `file:line` references, self-doubt each proposal, and drop anything that failed a no-false-positive filter. The agents produced 34 raw proposals; this document keeps 22 after validation. Items dropped or merged are listed in §8.

Every cited claim was spot-checked against the current source before inclusion. Specifically validated:

- `find_workspace()` is invoked 12 times in `application.rs::handle_control_request` (grep-confirmed at lines 1962, 1998, 2035, 2108, 2191, 2266, 2324, 2409, 2488, 2585, 2642, plus 105).
- `FrameReader::buf` field is allocated and cleared but never read into; the active buffer is a local `bytes: Vec<u8>`.
- `send_request` (no-timeout) is called from production at zero sites — only by `rpc_client::tests` and internally by `send_request_with_timeout`.
- `hook::run` is `async fn` but contains zero `.await` points — the runtime is overhead.
- `.helix/` chmod 0700 in `lifecycle.rs:22-30` runs unconditionally, including on pre-existing directories.
- `HandshakeOutcome` fields ARE now read (by `doctor::probe_initialize` at `doctor.rs:192-204`); the `#[allow(dead_code)]` on the enum is stale.

## 3. Scope and non-goals

**In scope:**

- Performance micro-optimizations to hook + dispatch hot paths (§4).
- Refactors that compress duplication and tighten the test suite (§5).
- Operational improvements that surface today's silent failures (§6).
- New MCP tools/resources that expose editor-unique data the LLM cannot otherwise reach (§7).

**Out of scope** (still — these were ruled out in the original spec §13 and remain ruled out):

- Native MCP inside Helix core.
- Steel-based MCP server.
- Kitty / tmux remote control.
- HTTP / SSE transport.
- Persistent bridge daemon (the bridge stays per-session stdio).
- Windows support (the bridge is Helix-fork-specific; cross-platform fixes only when they bleed into macOS/Linux behavior).

**Items deliberately not pursued in this phase but acknowledged:**

- Tree-sitter query tool (`helix_tree_sitter_query`). High implementation cost relative to LLM ergonomics — query syntax differs subtly across grammars and produces compile errors LLMs struggle to fix. Track in a future phase.
- Inlay hints, clipboard tools, save/close — adds nothing the existing surface or `helix_run_command` doesn't already cover.
- Macro/transaction wrappers — interesting but premature; ship primitives first, compose later.

## 4. Phase 7a — Performance optimizations

Six items, ordered by win/cost ratio. Total estimated impact in steady-state Claude Code usage: **~1–8 ms saved per MCP tool call** + **~50–100 µs saved per UserPromptSubmit**, with comparable reductions in syscall count and process-startup cost.

### 4.1 Hook: marker check before snapshot parse (P0)

`hook.rs::decide` currently reads and parses the snapshot file *before* consulting the per-session marker. The dominant case in steady state is "Claude is typing, user hasn't refocused Helix, the marker mtime matches the snapshot mtime" — exactly the case where we skip emission entirely. Today that skip pays for one full `read_to_string` + `serde_json::from_str::<ContextSnapshot>` first.

Reorder: after `metadata().modified()` produces the snapshot mtime, immediately compare against `read_marker_mtime(marker_path(session_id))`. If they match, return `Skip` without touching the file body.

**Win:** ~50–100 µs and 3 syscalls per UserPromptSubmit in steady state. With `include_buffer_text = true` (opt-in), the win scales linearly with buffer size.

**Cost:** ~10 LOC. Behavior preserved. Add one new test in `decide_tests` that writes a deliberately-malformed snapshot with a matching marker mtime and asserts `Skip("already injected this mtime for this session")`; if the malformed body is touched, the test panics on `from_str`.

**Self-doubt:** "The file is in page cache; you're micro-optimizing." Counter: every-prompt cost is the right multiplier; the right fix is the cheapest check first.

### 4.2 Hook: drop the tokio runtime entirely (P0)

`#[tokio::main]` in `helix-mcp/src/main.rs` builds a multi-threaded runtime for every subcommand. The hook subcommand contains zero `.await` points (verified) — it uses `std::fs::*`, `std::io::stdin`, `std::io::stdout`. The runtime is overhead.

**Approach:** Split `main`. `Command::Hook` becomes a synchronous path; `hook::run` drops its `async` signature. `Command::Serve` and `Command::Doctor` keep the runtime (they genuinely need async). Either build the runtime conditionally inside `main()` for the variants that need it, or move `#[tokio::main]` onto a `serve_or_doctor()` helper called from `main()`.

**Win:** ~0.5–1 ms saved per hook invocation (multi-threaded runtime startup including worker-thread spawn). Drops to ~150–300 µs if `flavor = "current_thread"` is preferred as a less-invasive intermediate. Three fewer kernel threads created per UserPromptSubmit.

**Cost:** ~20 LOC. Risk: low — the production hook code paths are sync already.

**Self-doubt:** "We might want to add tokio::fs later." Counter: tokio::fs is a thread-pool wrapper around blocking syscalls for small files — no advantage over plain `std::fs` for the hook's access patterns.

### 4.3 Snapshot writes: drop `sync_all` (P0)

`context_logger.rs:79` calls `f.sync_all()` before `fs::rename`. The `sync_all` is a full inode-data-and-metadata fsync. The spec's snapshot file is non-durable cache: readers degrade gracefully if the file is missing, and the next focus-loss or MCP mutation rewrites it. The atomic `tmp + rename` already gives readers all-or-nothing semantics; `sync_all` only protects against a power loss between the rename and the kernel writeback, which is exactly the case where the snapshot doesn't need to survive.

**Win:** ~0.5–5 ms saved per snapshot write. Snapshot writes happen on focus loss AND after every MCP mutation; a sequence of `open-file → goto-line → select` triggers three rewrites today. Combined with §4.4 below, the editor-side cost of an MCP mutation drops dramatically.

**Cost:** one-line change (`sync_all()` → no fsync, or replace with `sync_data()` if a reviewer wants a halfway measure). Add a test asserting the snapshot file appears after `write_context_file` returns Ok (the rename guarantees visibility).

**Self-doubt:** "fsync is a safety belt." Counter: the spec explicitly accepts snapshot loss; the file is regenerable. The fsync is overhead with no recovery benefit. POSIX `rename(2)` semantics already guarantee atomicity at the directory entry; on ext4/APFS with default journaling, the data-block ordering is also guaranteed without an explicit fsync.

### 4.4 Coalesce `find_workspace()` per dispatch (P0)

`handle_control_request` calls `helix_loader::find_workspace()` 12 times — once in each match arm — and `write_context_file` calls it again internally on mutations. Each call walks up to 6 ancestors × 4 markers (`.git`/`.svn`/`.jj`/`.helix`) = up to 24 stats.

**Approach:** Compute workspace once at the top of `handle_control_request`. Pass `&workspace` into the arms. For `write_context_file`, add a parallel API `write_context_file_with_workspace(editor, workspace, source, instance)` and have the existing one fall back to its own discovery for non-MCP callers (focus-lost path in ui/editor.rs).

**Win:** ~120–250 µs saved per tool call. ~24 syscalls saved per mutation tool call (where `write_context_file` would otherwise re-discover). Most user-visible on rapid Claude-driven tool chains like `open-file → goto-line → select`.

**Cost:** ~30 LOC. Either thread `workspace: &Path` through 5 helpers (cleaner) or memoize via a `OnceCell` on `Application` that invalidates on `:cd` (less invasive but introduces an invalidation rule).

**Self-doubt:** "The cwd is RwLock-cached in `helix-stdx`; stats are cheap." True for the inner cwd lookup; not for the 4-markers-per-ancestor scan that follows. A flamegraph would tell us whether this is real or noise; pending profiling, the duplication is at minimum a code-quality fix and at maximum a measurable win.

### 4.5 Cache socket discovery (P1)

`discovery::find_helix_socket` is invoked on every tool call (`serve.rs:319`). It does `tokio::fs::read_dir` of `.helix/` and a 200 ms-timed `UnixStream::connect` probe for every `control-*.sock` and `*.sock.path`. On a developer machine with accumulated orphans (4 stale + 1 live observed locally), every tool call pays for 5 connect probes plus the dirent walk.

**Approach:** Mirror the `HANDSHAKE_CACHE` pattern. Add `static SOCKET_CACHE: OnceLock<tokio::sync::Mutex<Option<PathBuf>>>` in `rpc_client.rs` (or its own module). On a tool call: if cache has a path AND `metadata(path)` shows it still exists, reuse. On transport error, invalidate (alongside the existing handshake invalidation). This is symmetric with how the handshake cache works today and follows the same invalidation discipline.

**Win:** ~50–500 µs saved per tool call depending on orphan count. ~10–15 syscalls → 1 (stat) on the warm path.

**Cost:** ~30 LOC. The invalidation hook already exists (`invalidate_handshake_cache` is called in dispatch_tool on transport errors); add a sibling `invalidate_socket_cache` at the same site.

**Self-doubt:** "Bridge is short-lived; cache adds no value." A Claude Code session can run 50+ tool calls before the bridge exits; even three benefit and we're net positive. The orphan-accumulation tail is the real concern (perf #4 and ops #3 from the audit converge here).

### 4.6 Snapshot: hash-and-skip when nothing changed (P2)

`write_context_file` rewrites the snapshot on every mutation, even when the serialized output is byte-identical to the existing file. Combine with §4.3 above: after the JSON payload is built, byte-compare against the existing file content (or, cheaper, a stored hash). If equal, skip the write entirely.

**Win:** ~30% of mutation-triggered rewrites estimated to be no-ops (cursor moves within the same column, format-document that's already formatted, etc.). Each skipped write saves ~0.5–1 ms even after §4.3 lands.

**Cost:** ~25 LOC. Needs a small `Application` field for the last-payload hash (or just re-read the file — depends on which is cheaper to maintain).

**Self-doubt:** Confidence MEDIUM. The 30% hit rate is unmeasured; if it's actually 5%, this isn't worth the complexity. Ship after §4.3 lands and measure.

### 4.7 Hook: bound the walk-up at the closest `.helix/` ancestor (P2)

`locate_snapshot` walks from `cwd` to the filesystem root when `.helix/context.json` isn't found, stat-ing `.helix/context.json` at every ancestor. In the no-bridge-configured case (most users on most prompts), this is up to 6+ failed stats per UserPromptSubmit.

**Approach:** Walk ancestors looking for a `.helix/` **directory** (one stat), and only then check the `context.json` inside it. If `.helix/` is found but `context.json` isn't, return None immediately (bridge is configured but Helix isn't running — no point walking further).

**Win:** ~5–20 µs saved per "no-bridge" UserPromptSubmit. Bigger win: bounds the walk so an accidental `~/.helix/context.json` from a previous session can't be picked up from a deep cwd.

**Cost:** ~10 LOC. Behavior change: today's code would find `~/.helix/context.json` from `/Users/angm/some/deep/nested/path/`; the proposed code stops at the first `.helix/` ancestor. Document this in the hook section of `helix-mcp/README.md`.

**Self-doubt:** A behavior change disguised as an optimization. The new behavior is arguably correct (cross-project context bleed was a real concern flagged in spec §10b before the workspace walk-up landed in `599b0ff8a`) but should be a deliberate decision, not a side effect of perf work.

---

**Phase 7a summary:** ~155 LOC total across 7 changes. Combined steady-state win in MCP tool calls: estimated 1–8 ms per call. Per-UserPromptSubmit savings: ~0.5–1.1 ms + 3 fewer kernel threads. Confidence HIGH on §4.1, §4.2, §4.3, §4.4; MEDIUM on §4.5, §4.6; LOW on §4.7's magnitude (correctness gain dominates).

## 5. Phase 7b — Refactors for maintainability

Three big-win refactors plus four small hygiene items. Behavior preserved across all; net LOC reduction estimated at ~500.

### 5.1 Extract per-method handlers from `handle_control_request` (P0)

`application.rs::handle_control_request` is one match expression with one arm per `ControlRequest` variant — ~870 lines, with five templates repeated across arms:

- "compute char index from (line, column)" — 5 copies of the same 6-line idiom
- "snapshot rewrite after mutation" — 6 copies of the same 8-line block, differing only by the method name in the log warning
- "resolve_buffer + reply-on-error" — 8 copies
- "find_workspace + project_root resolution" — 12 copies (see §4.4 for the perf side of this)
- "view-switch + set_selection + ensure_cursor_in_view_center" — 3 copies

**Approach:** Extract small private free helpers (or methods on `Application`) co-located in the same file — no new module, keeps this upstream-merge-friendly. Suggested:

- `fn snapshot_after_mutation(&self, method: &'static str)` — collapses the 6-copy block to a one-line call.
- `fn one_indexed_to_char(text: &Rope, line: usize, column: usize) -> usize` — collapses the 5-copy idiom.
- One `do_<method>` per variant (e.g., `do_get_hover`, `do_get_definition`) that returns `Result<ControlResponse, JsonRpcError>`; the outer match becomes a one-line dispatch.

**Win:** ~120–160 LOC removed from the central dispatch. Adding a new tool touches one location (one new helper) instead of being threaded through an 870-line match. The current `return` statements in arms (used to bypass the trailing `reply.send`) collapse to `?` returns inside helpers.

**Cost:** Half a day to a day. Mostly mechanical. Risk of regression: moderate — the LSP-position arms have subtle 0/1-indexing the integration tests cover behaviorally but not at the helper level. The `ensure_cursor_in_view_center` borrow-checker dance is documented inline at the existing call sites; the extraction must preserve the ordering.

**Self-doubt:** A reviewer could argue "small inline blocks are easier to upstream-merge than a refactor." Counter: the helpers go in our file — `application.rs` has already accumulated 4 bridge-helper free functions (`resolve_buffer`, `spawn_lsp_request`, `lsp_locations_to_schema`, `ensure_buffer_mode_safe`), none of which are upstream-tracked. Adding 2–3 more is consistent with the current shape.

### 5.2 Single source of truth for tool metadata (P0)

"Helix has tool X" is encoded in five places today:

1. `helix-context-schema/src/protocol.rs` — `ControlRequest` variant (wire type)
2. `helix-mcp/src/tools.rs` — `ToolKind` enum + 5 separate `match self { ... }` arms (name, description, input_schema, from_name, all)
3. `helix-mcp/src/serve.rs::call_tool` — 100 lines mapping tool args to `ControlRequest` variants
4. `helix-term/src/control_socket/dispatch.rs` — two hardcoded `Vec<String>` capability lists (read_methods, write_methods), plus enumeration of "needs editor loop" variants
5. `helix-mcp/README.md` + `serve.rs::SERVER_INSTRUCTIONS` — human-readable tool tables

Adding `helix_get_call_hierarchy` today requires touching: schema variant + 5 match arms in `ToolKind` + 1 arm in `serve.rs::call_tool` + 1 capability-list entry + 2 test-list assertions + README + `SERVER_INSTRUCTIONS`. Seven+ touchpoints.

**Approach:** A static table keyed by `ToolKind`:

```rust
struct ToolSpec {
    kind: ToolKind,
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
    parse_request: fn(Value) -> Result<ControlRequest, serde_json::Error>,
    is_mutation: bool,   // drives the read/write split in dispatch.rs
}
const TOOLS: &[ToolSpec] = &[...];
```

`ToolKind::name`, `description`, `input_schema`, `from_name`, `all` all become one-line lookups. `serve::call_tool`'s match collapses to `spec.parse_request(args_val)?`. `dispatch.rs::handle_initialize` derives its lists from `TOOLS` partitioned on `is_mutation`. Tests iterate `TOOLS` rather than hardcoded strings.

A compile-time exhaustiveness check confirms `TOOLS.len() == ToolKind::all().count()`.

**Win:** ~120 LOC removed across `tools.rs`, `serve.rs`, `dispatch.rs`. Adding a tool drops from 7+ touchpoints to 2 (schema variant + new `ToolSpec` entry). README/SERVER_INSTRUCTIONS still hand-maintained — those are user-facing prose, and the rest of the system isn't auto-generated text.

**Cost:** Half a day. Behavior preserved.

**Self-doubt:** "Arg structs (HelixOpenFileArgs etc.) are documentation; flattening loses that." Counter: keep the arg structs unchanged — the table just removes dispatch boilerplate. "The dispatch.rs lists are intentional; read vs write is a real distinction." Counter: encode it in the `is_mutation: bool` field.

### 5.3 Test harness for fake-helix integration tests (P0)

`helix-mcp/tests/integration.rs` has 15 nearly-identical tests. Each: TempDir + write `SAMPLE_SNAPSHOT` + `Command::new(binary_path())` + 6 lines of stdio piping + 3-message init + 8-line read-loop + substring assertion. Roughly 50 LOC each, of which 10 are unique.

Also, the assertions are loose. `tools_call_run_command_against_fake_helix` asserts `line.contains("context snapshot written")` — but the fake-helix always returns that string regardless of what request was sent; the assertion passes even if the bridge corrupts the request and the fake's canned response just echoes by accident.

**Approach:** Add `helix-mcp/tests/common/mod.rs` with a `Harness` struct that bundles tempdir + spawned binary + stdio + handshake. Tests become 5–10 LOC each, with assertions on parsed JSON (`result["content"][0]["text"]` parsed back to a `Value`) rather than substring.

**Win:** ~200–300 LOC saved across 15 tests. New tests = 3 lines. Strict assertions catch the "fake echoes canned regardless" hole that today's substring matches paper over.

**Cost:** Half a day for the harness + mechanical conversion. Test-only code; production paths untouched.

**Self-doubt:** "Each test is self-contained; harness adds indirection." Counter: the read-jump cost is paid once for a new reader; the boilerplate cost is paid every test addition. The integration count grew from 5 (Phase 4a) to 15 today; the trend justifies the harness.

### 5.4 Small hygiene items (P2)

Four small fixes that don't individually justify their own subsection but should land together as a hygiene pass:

- **Drop stale `#[allow(dead_code)]` on `HandshakeOutcome`** (`rpc_client.rs:113-128`). Fields ARE read by `doctor::probe_initialize`; the comment "future diagnostics will display the version strings" is stale (the future is now). Removing the `allow` restores warning hygiene for any new unused field added later.
- **Delete `FrameReader::buf`** (`control_socket/framing.rs:13-23`). The field is allocated, cleared on each call, and never written to — the read loop builds into a local `bytes: Vec<u8>`. Refactor remnant.
- **Make `send_request` private** (`rpc_client.rs:43`). Phase 6b made `send_request_with_timeout` mandatory for production; the no-timeout version remains `pub` but has zero production callers (verified). Tests in the same module keep direct access. Closes a footgun.
- **Centralize `TEST_ENV_LOCK`'s `.unwrap_or_else(|e| e.into_inner())` idiom** into a tiny helper. Rename `HANDSHAKE_TEST_LOCK` (in `rpc_client.rs::tests`) to `HANDSHAKE_CACHE_LOCK` — it guards the process-global handshake cache, not env vars, and the misnaming sends future readers in the wrong direction.

**Win:** ~50 LOC trimmed total, plus restored compile-time hygiene. Each is < 30 minutes.

**Self-doubt:** Below the "≥ 50 LOC saved" filter individually. Bundle them together to clear the bar.

### 5.5 Log level discipline (P2)

`log::warn!` and `log::debug!` are used inconsistently for the same kind of event. Most notably, `hook.rs:316` logs `warn!` for the fence-escape skip (a benign, expected, working-as-intended security behavior) — anyone with the bridge's spec doc open and a selection across the fence example will get a `warn` every prompt. Conversely, `hook.rs:260` logs `debug!` for legitimate skip-because-marker-matches, which is the case operators most want to see when debugging.

**Approach:** Document a two-line policy at the top of `hook.rs`:

- `debug!` for expected control-flow events (skip-because-marker, skip-because-mcp_command).
- `warn!` for unexpected recovered failures (failed I/O on emit, failed marker write).
- `error!` for things the user must see.

Then audit the 7 sites in `hook.rs` and the matching sites in `control_socket/server.rs` (`frame read error`/`write error` at `warn!` are expected on client disconnect; downgrade to `debug!`).

**Win:** Quality of `RUST_LOG=warn` output goes from "noisy on benign edge cases" to "meaningful." Operational quality, not LOC.

**Cost:** Half an hour. Behavior unchanged (logs only affect operators).

**Self-doubt:** Stylistic. Included because the line 316 case has a concrete repro (selection across the bridge spec → warn every prompt) and because the audit's category 4 explicitly covers log-level discipline.

---

**Phase 7b summary:** ~500 LOC net reduction across three big refactors plus a hygiene pass. Confidence HIGH on §5.1, §5.2, §5.3 (mechanical wins); MEDIUM on §5.4, §5.5 (operational quality, harder to quantify).

## 6. Phase 7c — Operational maturity

Six items addressing observability, deployment, and silent-failure surfacing.

### 6.1 Surface control-socket startup failures in the editor status bar (P0)

A user enables `[editor.control-socket]` but Helix can't bind — EADDRINUSE, EACCES on `.helix/`, sun_path overflow, no workspace marker. `start_control_socket` logs `warn!` and continues. Helix starts normally. Claude calls a tool, gets "Helix is not running in this workspace." The user has been editing the entire time and has no idea.

**Approach:** After the `log::warn!` in `application.rs:720`, also `editor.set_error(format!("Control socket failed to start: {}. Run `helix-mcp doctor` for details.", e))`. The message lands in the editor status bar, matching how LSP/DAP/debugger startup failures already surface.

**Win:** Closes the most common "Claude can't see my editor and I don't know why" failure mode.

**Cost:** 3 lines.

**Self-doubt:** "User opted in; they should read logs." Every other Helix subsystem that fails to start surfaces a status error. This is house-style consistency.

### 6.2 `helix-mcp doctor` validates config files (P1)

Doctor today verifies binary on PATH, workspace resolvable, snapshot present, socket connectable, handshake. It does NOT check whether `[editor.context-logger]` or `[editor.control-socket]` are enabled in `~/.config/helix/config.toml`, whether `.mcp.json` exists and references `helix-mcp`, or whether `~/.claude/settings.json` has the hooks wired.

**Approach:** Add a config-sanity section to doctor that parses (best-effort) the three config files and reports what's enabled vs what's missing. Cross-check: if `.mcp.json` references the bridge but `[editor.control-socket] enabled = false`, say so explicitly.

**Win:** Onboarding pain falls. Every new user today has to consult three config files in three formats; doctor's job is to short-circuit that.

**Cost:** ~150 LOC + tests. TOML and JSON parsing are already in the dep tree via serde.

**Self-doubt:** Adds scope to doctor. Ship in two passes if needed: first the parse-and-report, later the cross-checks.

### 6.3 `serve` subcommand `--verbose` flag + structured log targets (P1)

`Phase 6b` shipped `--verbose` on the hook subcommand. The `serve` subcommand — the long-lived MCP server handling every tool call — has no equivalent. Debugging "why did this tool call fail" requires editing `.mcp.json` to set `RUST_LOG=debug`, restarting Claude Code, then parsing interleaved logs.

**Approach:** Two parts.

- Add `--verbose` to `Command::Serve` (and a sibling `HELIX_MCP_VERBOSE=1` env var that the hook also honors). When set: log every `dispatch_tool` call with a per-call ID (monotonic counter), tool name, and request shape preview. Log handshake outcome and discovery results.
- Audit the 14 call sites of `log::warn!`/`log::debug!` and add `target: "helix_mcp::dispatch"`, `target: "helix_mcp::discovery"`, etc. The spec's Phase 6 telemetry note (§11) already specifies `target: helix_term::context_logger` and `target: helix_term::control_socket`; the code drifted from this.

**Win:** Operators can run `RUST_LOG=helix_mcp::dispatch=debug,helix_mcp::discovery=info` to scope debug output. Per-call correlation makes "Claude says Helix is unreachable" tractable.

**Cost:** ~50 LOC for the verbose flag + 14 mechanical target edits.

**Self-doubt:** "Just use `RUST_LOG=debug`." That requires editing `.mcp.json` and restarting Claude Code per session — friction high enough that nobody does it.

### 6.4 Reap stale marker files (P1)

The hook writes `marker-<session_id>` per Claude Code session. There's no delete path when a session ends — only `--reset-marker` on PostCompact and SessionStart=compact. Markers accumulate. Reproduced locally: 12 files after 2 days of normal use.

**Approach:** On the `Emit` success path in `hook::run`, opportunistically prune markers older than 7 days. Cheap glob + stat. Best-effort; errors logged at debug.

**Win:** Bounded marker-file count over time. On macOS where the marker dir is `~/Library/Caches/claude-helix/` (no tmpfs reboot cleanup), this matters in the long term.

**Cost:** ~30 LOC + test.

**Self-doubt:** Functional impact is zero — the files are inert. A reviewer could reasonably say "wait until someone complains." On the other hand, the fix is local and one-shot.

### 6.5 Don't chmod 0700 on pre-existing `.helix/` (P1)

`lifecycle.rs:22-30` unconditionally creates `.helix/` and chmods it 0700. If `.helix/` pre-existed with different permissions (e.g., a team that commits `.helix/config.toml` and shares with mode 0755 so teammates can read), Helix silently downgrades it. On NFS / Docker volumes that don't honor mode bits, `set_permissions` returns success but the actual permissions stay at the filesystem default — silently inconsistent.

**Approach:**

- Only chmod 0700 if `create_dir_all` actually created the dir (was-absent → now-present). Probe with `parent.exists()` before the call.
- On bind failure, roll back: if we created `.helix/` and it's empty, remove it. Avoids leaving a mystery directory the user didn't ask for.

**Win:** Closes the "Helix downgrades my team-shared `.helix/` permissions" surprise. Closes the "bind failed and now there's a mystery directory" surprise.

**Cost:** ~20 LOC + 2 tests.

**Self-doubt:** "Mode 0700 on .helix/ IS what most users want." True; that's why this change is conservative — only honor the user's pre-existing choice if they made one.

### 6.6 Two-instance discovery: deterministic tiebreak on PID (P2)

When two Helix instances in the same workspace have sockets bound within the same filesystem mtime tick, `find_helix_socket`'s sort-by-mtime is non-deterministic — `dirent` order varies by filesystem. macOS picks one; Linux CI picks another.

**Approach:** Tiebreak on PID parsed from the socket filename — newest mtime, then highest PID. Five lines + a unit test.

**Win:** Determinism for an edge case that's already defined in spec §7.4. Cost is trivial.

**Cost:** ~10 LOC.

**Self-doubt:** Honest edge case. Worth shipping only because the fix is cheap.

---

**Phase 7c summary:** ~270 LOC across 6 changes. Most impact: §6.1 (status bar surfacing) and §6.3 (debugging story for `serve`).

## 7. Phase 7d — New tools and resources

Nine new MCP tools/resources that meaningfully expand what AI coding agents can do *because* they're paired with a live editor. Each was filtered against "does this beat Claude's existing Read/Edit/Grep/Bash ergonomics with comparable cost." Items that lose this comparison are listed in §8.

### 7.1 `helix_get_selection` (P0)

Return the rope-extracted text and 1-indexed ranges of every selection in the current view.

**Why not redundant:** The snapshot carries cursors and selections as `(line, column)` pairs, no text. Today Claude must read the file from disk and slice — wrong when the buffer is modified-but-unsaved. With Helix open, the rope is authoritative.

**Wire:** `ControlRequest::GetSelections { path: Option<String> }` → `ControlResponse::GetSelections { selections: Vec<SelectionWithText>, primary_index: usize, mode: String }` where each `SelectionWithText { anchor: Position, head: Position, text: String, is_primary: bool }`. Cap text at 64 KiB per range with a truncation marker.

**MCP:** `helix_get_selection(path?)`. SERVER_INSTRUCTIONS line: "Use when the user says 'fix the selected region' or 'rename what I highlighted' — returns the live rope content, not the disk content."

**Helix-side:** `doc.selection(view_id)` + `rope.slice(range.from()..range.to())`. Selection-to-Position conversion already exists in `context_logger.rs`.

**Risk:** Multi-selection text can be large; the per-range cap covers it.

**Priority:** HIGH. Cheap, high-leverage, eliminates a class of "Claude reads stale disk" bugs.

### 7.2 `helix_multi_select` (P0)

Replace the current Selection with N ranges. First range becomes primary; view recenters on it.

**Why not redundant:** `helix_select` ships a single range. Multi-selection is the entire reason Helix exists; without a multi-select primitive, Claude can't demonstrate the structural edits Helix is good at ("select all `Foo::new(...)` calls, then `c` to replace").

**Wire:** `ControlRequest::SelectMulti { ranges: Vec<RangeSpec>, primary_index: Option<usize>, path: Option<String> }` → existing `Ok {}`. `RangeSpec { start_line, start_column, end_line, end_column }`, 1-indexed inclusive.

**MCP:** `helix_multi_select(ranges, primary_index?, path?)`.

**Helix-side:** `helix_core::Selection::new(SmallVec, primary)`. The single-range path in `SelectRange` is the template — generalize the `pos_to_char` closure, build N ranges, pass to `Selection::new`.

**Risk:** Empty ranges → InvalidParams. Overlapping ranges → Helix's Selection auto-merges via `normalize()`; document so Claude knows N inputs may become N-k outputs.

**Priority:** HIGH. Cheap; unlocks a category Helix uniquely owns.

### 7.3 `helix_buffer_read` (P0)

Read text from the buffer's rope (live, includes unsaved edits), optionally line-ranged.

**Why not redundant:** The wire method `GetBufferText` already exists in the protocol — it was never exposed as an MCP tool. The unique value: rope = unsaved edits; Read = disk. Today when a user has typed in Helix and pressed prompt-submit before saving, Claude sees stale content.

**Wire:** Already exists. `ControlRequest::GetBufferText { path: Option<String>, range: Option<LineRange> }` → `ControlResponse::GetBufferText { text: String, language: Option<String>, line_count: usize }`. Zero wire change.

**MCP:** `helix_buffer_read(path?, start_line?, end_line?)`. Mention in SERVER_INSTRUCTIONS that it should be preferred over Read when the buffer may be modified.

**Helix-side:** Handler already implemented. Work is purely in `helix-mcp/src/tools.rs` (new `ToolKind`) and `serve.rs` dispatch.

**Risk:** Claude could over-prefer this and skip Read entirely. Mitigate via description ("use only when the buffer may be modified").

**Priority:** HIGH. Lowest cost on this list (no wire change), real value (closes the unsaved-edits class of failures).

### 7.4 `helix_get_document_symbols` (P0)

LSP-backed structured outline of a file. Returns nested symbol tree (functions, methods, types, classes) with ranges and kinds.

**Why not redundant:** `helix_get_workspace_symbols` is fuzzy global search; `helix_get_document_symbols` is the outline of one file. Grep approximates with `^fn ` regex but misses indented methods, doc comments, language-specific structure. LSP-backed symbols are accurate and require an LSP — which is already running in Helix.

**Wire:** `ControlRequest::GetDocumentSymbols { path: Option<String> }` → `ControlResponse::GetDocumentSymbols { symbols: Vec<DocumentSymbol> }` where each `DocumentSymbol { name, kind, range, selection_range, container_name, children }` (nested form).

**MCP:** `helix_get_document_symbols(path?)`.

**Helix-side:** `language_server.document_symbols(doc.identifier())` — already exists. Pattern matches the existing `GetWorkspaceSymbols` handler.

**Risk:** LSPs vary between flat and nested forms; the handler must accept both.

**Priority:** HIGH. Reuses an existing LSP call; cheap mirror of the workspace-symbols handler.

### 7.5 `helix_get_code_actions` + `helix_apply_code_action` (P1)

List LSP code actions at a position/range; apply one by ID.

**Why not redundant:** Code actions are the editor's structured response to "fix this." Quick-fixes for diagnostics, refactors, organize-imports — all derived by the LSP from semantic analysis. Claude cannot re-derive these from a diagnostic message alone.

**Wire:** Two new methods. `ControlRequest::GetCodeActions { line, column, end_line?, end_column?, path?, only?, allow_insert_mode? }` → `ControlResponse::GetCodeActions { actions: Vec<CodeActionDescriptor> }`. Then `ControlRequest::ApplyCodeAction { action_id }` → `ControlResponse::ApplyCodeAction { applied: bool, message: Option<String> }`. `action_id` is opaque, stamped by Helix with `(doc_id, doc_revision)` so a stale apply after the buffer changed gets rejected cleanly.

**MCP:** `helix_get_code_actions(line, column, path?, end_line?, end_column?, only?)` + `helix_apply_code_action(action_id)`. SERVER_INSTRUCTIONS: "When fixing a diagnostic, first call get_code_actions at the diagnostic position. If a quickfix is offered, apply it instead of hand-editing — the LSP knows the exact transform."

**Helix-side:** `language_server.code_actions(...)` exists at `helix-lsp/src/client.rs:1680`. Apply path reuses `helix-term/src/commands/lsp.rs::apply_workspace_edit` (confirm visibility during implementation).

**Risk:** Two-call pattern introduces stale state; the doc-revision stamp closes that hole. Surprise factor: LSP edits can be large; the user sees them land via Helix's transaction so undo is one keystroke.

**Priority:** HIGH (big leverage), medium cost.

### 7.6 `helix_get_signature_help` (P2)

LSP function-signature help at a position. Returns overload list + active parameter index.

**Why not redundant:** Hover gives doc comments; signature help gives call shape with the cursor's active parameter highlighted. Different LSP method; the editor knows which overload the cursor is at given surrounding context.

**Wire:** `ControlRequest::GetSignatureHelp { line, column, path?, allow_insert_mode? }` → `ControlResponse::GetSignatureHelp { signatures, active_signature?, active_parameter? }`.

**MCP:** `helix_get_signature_help(line, column, path?)`. Note: signature help is most useful mid-typing, so `allow_insert_mode` should default to `true` for this method (override the standard refusal).

**Helix-side:** `language_server.text_document_signature_help(...)`. Identical pattern to GetHoverAt.

**Risk:** Pretty low. The Insert-mode exception is unusual but justified (LSP designed for mid-typing).

**Priority:** MEDIUM.

### 7.7 `helix_get_jumplist` + `helix_jump` (P2)

Return the current view's jumplist; jump back/forward N entries.

**Why not redundant:** Jumplist is *intent history* — where the user has been. When Claude is asked "go back to where I was looking", there's no other source. Snapshot has the active buffer; jumplist has the trail.

**Wire:** `ControlRequest::GetJumplist {}` → `ControlResponse::GetJumplist { entries: Vec<JumpEntry>, current_index: usize }`. Plus `ControlRequest::Jump { offset: i32 }` → existing `Ok {}`.

**MCP:** `helix_get_jumplist()` + `helix_jump(offset)`.

**Helix-side:** `view.jumps` (`helix-view/src/view.rs`). Read-only get + `jumps.move_to` for jump.

**Risk:** Per-view; default to focused view, allow `path` override later.

**Priority:** MEDIUM. Cheap; modest value.

### 7.8 MCP notification stream (P2)

Push notifications when the active buffer changes or diagnostics update. Uses MCP's existing `notifications/resources/updated`.

**Why not redundant:** Today Claude polls `helix_get_diagnostics` and may get stale answers; either over-polls (cost) or under-polls (staleness). Notifications are MCP's designed answer.

**Wire change:** This is the largest wire change in the list. Today the protocol is strict request/response — adding server-initiated frames requires:

- New `ControlNotification` enum (`DiagnosticsChanged { path, count }`, `ActiveBufferChanged { from, to }`).
- A subscription handshake (`ControlRequest::Subscribe { topics: Vec<String> }` → `Subscribe { subscription_id }`).
- Framing support for unsolicited notification frames from Helix to the bridge.

**MCP:** Bridge emits `notifications/resources/updated` for `helix://state/current` on active-buffer change, and for a new parameterized resource `helix://state/diagnostics?path=...` on diagnostics change.

**Helix-side:** Hook into existing `DiagnosticsDidChange` event and a new `DocumentFocusChanged` event (may need adding). Route through a new per-subscription channel.

**Risk:** Largest architectural change in this document. Bridge connection state grows (subscriptions). Conservative ship: just `ActiveBufferChanged` (single event, low cost), defer `DiagnosticsChanged` until measured.

**Priority:** MEDIUM. Real value; meaningful cost. Ship the conservative single-event v1.

### 7.9 `helix_open_diff` + `helix_close_diff` (P3)

Open a unified-diff scratch buffer for a proposed edit, before applying it via Claude's Edit/Write tools.

**Why not redundant:** Navigate-before-edit (already shipped) shows where the change lands; diff preview shows what it will look like. Closest the bridge can come to a real "AI pair programmer in the editor" loop.

**Wire:** `ControlRequest::OpenDiff { path, new_content, label? }` → `ControlResponse::OpenDiff { diff_buffer_id }`. Plus `ControlRequest::CloseDiff { diff_buffer_id }` → existing `Ok {}`.

**MCP:** `helix_open_diff(path, new_content, label?)` + `helix_close_diff(diff_buffer_id)`.

**Helix-side:** Compute a unified diff (re-use `helix-vcs/src/diff/` machinery or `similar`); open a scratch buffer; split the window. Buffer-id mapping on Application.

**Risk:** Most opinionated proposal — invents a workflow rather than exposing a primitive. Could be rejected as scope creep.

**Priority:** LOW-MEDIUM. Real user value; higher cost. Consider as a Phase 8 follow-up once primitives ship.

---

**Phase 7d summary:** 9 tools across 7 wire methods (some pair up). The first four (§7.1–7.4) are clear net wins with low cost; §7.5 is high-leverage with medium cost; §7.6–7.8 are lower-priority but reasonable; §7.9 is the speculative one.

## 8. Items considered and dropped

Recorded for accountability. Each was raised by an agent and dropped during synthesis.

**Performance:**

- "Trim path/lang allocations in `build_snapshot`" — agent acknowledged 1–3 µs/snapshot, marginal. Bundle into §5.1 (extract handlers) if convenient; not its own item.
- "Sync pointer read in discovery" — agent flagged the lowest fish. Subsumed by §4.5 (socket cache makes the pointer follow a one-time cost per process).

**Features:**

- `helix_tree_sitter_query` — real capability gain but query-syntax friction makes Claude struggle without good error feedback. Defer.
- `helix_save_buffer` / `helix_close_buffer` — covered by `helix_run_command("write")` / `:bclose`.
- `helix_clipboard_get/set` — privacy concern + Claude has no legitimate need.
- `helix_get_inlay_hints` — decoration only; hover suffices for type info.
- `helix_undo` / `helix_redo` — dangerous to give the LLM unilaterally.
- `helix_buffer_diff_vs_disk` — niche; `helix_buffer_read` + comparing to file is sufficient.
- `helix_get_lsp_servers` — niche debugging info.
- `helix_get_view_tree` — unclear what Claude would do with split-layout info.
- Macro/transaction wrappers — premature; ship primitives first.

**Code quality:**

- "ToolKind via `strum`" — adds a dep for one feature handwritten matches cover.
- "`spawn_lsp_request`'s `convert` closure as a trait method" — readability regression for 4 call sites.
- "Cache `is_destructive_typable_command`'s env check" — nanoseconds vs cache-invalidation risk; not worth it.
- "`Resolved` struct in `path.rs` as an enum" — touches upstream-mergeable code in non-trivial ways.

**Operational:**

- "Two-instance handshake cache mixing" — fork-of-the-mind; each `helix-mcp serve` is its own process with its own cache. Not a real bug.
- "Helix process zombie" — `is_socket_live`'s connect probe handles it.
- "Snapshot half-written" — atomic tmp+rename already covers it.
- "Symlinked workspaces" — `resolve_buffer` canonicalizes before comparison.
- "WSL2 socket-path overflow" — pointer-file fallback handles it.

## 9. Risk register (Phase 7-specific)

| Risk | Severity | Mitigation |
|---|---|---|
| §4 perf wins regress correctness | Medium | Each item has a regression test in its acceptance criteria; the fsync removal in §4.3 is the most fragile (the test must assert visible-after-rename behavior on the target filesystem). |
| §5.1 handler refactor breaks subtle 0/1-indexing | Medium | Integration tests in `helix-mcp/tests/integration.rs` cover behavioral correctness; converting them to the §5.3 harness with strict JSON assertions catches more. |
| §5.2 single-source table accidentally drops a tool | Low | Compile-time `assert_eq!(TOOLS.len(), ToolKind::all().count())` via an exhaustiveness check. |
| §7 new tools land before §5 refactors → 7-place edits | Low | Sequence §5.1 + §5.2 before §7.4–7.9 so the new tools land in the table shape, not the duplicated shape. |
| §7.8 notification stream wire-format change breaks existing clients | Medium | Notifications are additive; old clients ignore unknown frames. Bridge stays backward-compatible by default; the new `Subscribe` method gates the subscription state machine. |
| §6.3 verbose-mode logs leak sensitive state | Low | The verbose breadcrumbs include first 200 bytes of request payloads; clip to method names + arg keys for any future PII-sensitive context. |

## 10. Suggested merge order

A practical sequencing that minimizes rebase pain and lets each ship independently:

**Sprint 1 (perf + hygiene quick wins):**

1. §4.3 (drop sync_all) — one-line, biggest win.
2. §4.1 (marker before parse) — ten lines.
3. §4.2 (drop tokio for hook) — refactor but well-bounded.
4. §5.4 (small hygiene items).

**Sprint 2 (refactor foundation):**

5. §5.2 (tool metadata table) — unlocks low-touchpoint additions for new tools.
6. §4.4 (workspace coalesce) — pairs with §5.1 since both touch `handle_control_request`.
7. §5.1 (extract handlers).
8. §4.5 (socket cache) — pattern already exists for handshake.
9. §5.3 (test harness) — gates the next sprint's confidence.

**Sprint 3 (operational + high-priority features):**

10. §6.1 (status bar on bind failure).
11. §6.3 (verbose serve + log targets).
12. §6.4 + §6.5 + §6.6 (marker reaping, .helix permissions, mtime tiebreak).
13. §7.3 (buffer_read) + §7.1 (get_selection) + §7.4 (document_symbols) — the cheap-and-clear set.

**Sprint 4 (new wire methods + bigger features):**

14. §7.2 (multi_select).
15. §7.5 (code_actions + apply).
16. §7.6 (signature_help).
17. §7.7 (jumplist).
18. §6.2 (doctor config-file checks).

**Phase 8 (speculative / opinionated):**

19. §7.8 (notification stream).
20. §7.9 (diff preview).

## 11. Self-doubt and criticism

Things in this document I'm uncertain about and a reviewer should challenge:

1. **The performance numbers are estimates, not measurements.** Every win in §4 cites "estimated µs" or "estimated ms" derived from syscall counting and rough per-syscall costs, not a profiler in hand. If §4.3 (sync_all removal) actually saves 50 µs instead of 5 ms, the analysis still holds (no recovery cost), but the prioritization argument weakens. Recommend: before sprint 1 lands, run `hyperfine` or perf against a synthetic `open-file → goto-line → select` chain to validate the win bands.

2. **§5.2 (tool metadata table) is the highest-leverage refactor but also the riskiest to do badly.** A bad table shape locks in awkwardness for every future tool. The proposed `parse_request: fn(Value) -> Result<ControlRequest>` is workable but requires each tool's arg-struct deserializer to be a function pointer, which is awkward when the struct has lifetime parameters or generic args. Verify during implementation that all 10 current tools fit the `fn` signature cleanly; if not, the table grows a per-tool inline closure and the win shrinks.

3. **§5.1 (handler extraction) might be the wrong shape.** I proposed free helpers; an alternative is a trait `ControlRequestHandler` with one method per variant, dispatched by `match`. Trait-based is more "Rust idiomatic" but adds a layer of indirection for no real win. Going with free helpers because they're cheaper to undo if the experiment fails.

4. **§6.4 (marker reaping) might cause silent injection regressions.** If a user has a long-running session that exceeds the 7-day threshold (e.g., a screen session left open), the marker disappears under them and the next prompt unexpectedly re-injects. Mitigation: thread the threshold through config, default to 7 days. Mention in the spec when shipping.

5. **§7.5 (code actions) has a UX trap.** Claude calling `helix_apply_code_action` with a stale `action_id` (the buffer changed between get and apply) gets a clean error — but what should Claude do? Re-run get, find the equivalent action by title? That's fuzzy. The doc-revision stamp prevents footguns but doesn't give Claude a recovery path. May warrant a follow-up "list-and-apply atomically" tool that picks by title in one shot.

6. **§7.8 (notification stream) is the largest architectural change.** Today the wire protocol is strict request/response; adding server-initiated frames means every connection grows a state machine. The conservative single-event v1 mitigates this — but if v1 ships and we never bother with v2, we've added complexity for one event. Verify there's a real demand before building this.

7. **The audit ran on `nightly`, which is 143 commits ahead of `fork/master`.** Some of the cited fragility is upstream-Helix code we touched only lightly (`application.rs::handle_control_request` is upstream-tracked even though the bridge logic is fork-only). The §5.1 refactor must stay inside the bridge-extracted helpers — touching upstream patterns increases the merge-conflict surface for the daily sync workflow.

8. **`helix_open_diff` (§7.9) is opinionated UX dressed as a tool.** The bridge has so far been a thin shim: expose primitives, let the LLM compose workflows. A diff-preview tool invents a workflow. If the workflow doesn't catch on among users, it's dead code. Defer pending evidence of demand.

9. **The "code quality" agent and the "performance" agent both proposed socket-discovery caching (§4.5 ↔ ops #3) without explicit cross-talk.** That convergence reinforces the proposal but also means I'm trusting one finding across two angles. A counter-angle: maybe the discovery cost is in fact negligible and both agents over-weighted it.

10. **Nothing here changes what the bridge fundamentally is.** Phase 7 is a maturity pass — sharper edges, more capability, less duplication. After all this lands, the bridge is the same shape: a per-session stdio MCP server bridging Helix's control socket to any agent. If someone has a vision for "what the bridge should become" rather than "what it should be better at," none of these tasks deliver on that. That's a separate conversation.

## 12. What this document doesn't cover

- **Specific implementation plans for each item.** This is a design spec. Bite-sized task breakdowns belong in `docs/plans/2026-MM-DD-phase-7<x>-...md`, following the `superpowers:writing-plans` skill's TDD format. Recommend one plan per sub-phase (7a, 7b, 7c, 7d) once the sequencing in §10 is approved.
- **Cross-agent rollout.** The new tools in §7 benefit any MCP client (Claude Code, Codex, Cursor, Cline, Continue, Zed). No agent-specific work; the SERVER_INSTRUCTIONS update handles capability discovery.
- **Backward compatibility with old `helix-mcp` binaries against new Helix builds (and vice versa).** The wire protocol is at v1 and stays at v1 throughout Phase 7 — only `SelectMulti`, `GetDocumentSymbols`, `GetCodeActions`, `ApplyCodeAction`, `GetSignatureHelp`, `GetJumplist`, `Jump`, `OpenDiff`, `CloseDiff`, `Subscribe` are added. Older clients ignore unknown methods; older Helix builds reject unknown methods with `MethodNotFound`, which the bridge surfaces cleanly. No version bump needed unless §7.8 changes the framing.

## Appendix A — Sources

- Phase 6 design spec: `docs/specs/2026-05-12-helix-mcp-bridge-design.md`
- Phase 6b plan: `docs/plans/2026-05-14-phase-6b-deferred-items.md`
- Agent reports: four parallel runs against `nightly` HEAD at commit `ec7ea1aa2`, dispatched 2026-05-14. Reports captured 34 raw proposals; 22 kept after validation, 12 dropped (§8).
- Spot-checked citations: `find_workspace()` × 12 in application.rs, `FrameReader::buf` dead, `send_request` only in tests, `hook::run` zero awaits, `.helix/` chmod 0700 unconditional, `HandshakeOutcome` fields read by doctor.
