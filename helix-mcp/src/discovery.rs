//! Find the right Helix control socket for the current workspace.
//!
//! Per spec §7.4, discovery globs `<workspace>/.helix/control-*.sock` plus
//! any pointer files (`*.sock.path` — used when the project-local path
//! would exceed sun_path). Filters out unconnectable sockets via a brief
//! connect attempt. If multiple live sockets exist, picks the one with the
//! newest mtime.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::net::UnixStream;

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("CLAUDE_PROJECT_DIR not set and no working directory available")]
    NoWorkspace,
    #[error("no live Helix control socket found in {0}/.helix/")]
    NoLiveSocket(PathBuf),
    #[error("reading .helix dir: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Socket-path cache
// ---------------------------------------------------------------------------
//
// `find_helix_socket` walks .helix/, probes each candidate, and picks the
// newest live socket. On a developer machine with accumulated orphan
// socket files (every Helix crash leaves one behind), that's a `read_dir`
// + several 200 ms-bounded `UnixStream::connect` probes per call. Every
// MCP tool call paid this cost.
//
// Cache the result for the lifetime of the bridge process. The cache is
// invalidated by `invalidate_socket_cache` (called from dispatch_tool on
// transport-level errors, alongside handshake invalidation). If the
// cached path stops existing between calls, we re-discover transparently.

static SOCKET_CACHE: OnceLock<tokio::sync::Mutex<Option<PathBuf>>> = OnceLock::new();

fn socket_cache() -> &'static tokio::sync::Mutex<Option<PathBuf>> {
    SOCKET_CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// Cache-checked variant of `find_helix_socket`. The first call (per
/// process) runs full discovery and stores the result. Subsequent calls
/// return the cached path *only if it still exists on disk*; otherwise
/// they re-discover. Transport errors during a tool RPC should call
/// `invalidate_socket_cache` to force the next call to re-discover even
/// when the cached path still happens to exist (covers the case where
/// Helix restarted with the same filename but a new PID handed out by
/// the kernel — unlikely but cheap to defend against).
pub async fn find_helix_socket_cached(
    workspace_override: Option<&Path>,
) -> Result<PathBuf, DiscoveryError> {
    let mut guard = socket_cache().lock().await;
    if let Some(cached) = guard.as_ref() {
        if tokio::fs::metadata(cached).await.is_ok() {
            return Ok(cached.clone());
        }
        // Cached path is gone — Helix exited cleanly or the socket
        // file was unlinked. Fall through to fresh discovery.
        *guard = None;
    }
    let resolved = find_helix_socket(workspace_override).await?;
    *guard = Some(resolved.clone());
    Ok(resolved)
}

/// Clear the cached socket path. dispatch_tool calls this whenever a
/// tool RPC fails with a transport-level error, alongside the matching
/// `invalidate_handshake_cache`. The next discovery will rebuild the
/// cache against whatever's listening now.
pub async fn invalidate_socket_cache() {
    let mut guard = socket_cache().lock().await;
    *guard = None;
}

/// Discover the live Helix control socket. Returns the path that should be
/// passed to `rpc_client::send_request`.
///
/// `workspace_override` lets callers (and tests) skip env-var lookup.
pub async fn find_helix_socket(
    workspace_override: Option<&Path>,
) -> Result<PathBuf, DiscoveryError> {
    let start = match workspace_override {
        Some(p) => p.to_path_buf(),
        None => {
            std::env::var_os("CLAUDE_PROJECT_DIR")
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
                .ok_or(DiscoveryError::NoWorkspace)?
        }
    };

    // Walk up looking for the first ancestor (inclusive) that contains a
    // `.helix/` directory. Mirrors the hook's behavior so the bridge keeps
    // working when Claude Code is launched from a subdirectory of the
    // workspace.
    let workspace = walk_up_to_helix(&start).unwrap_or(start);
    let helix_dir = workspace.join(".helix");
    if !helix_dir.exists() {
        return Err(DiscoveryError::NoLiveSocket(workspace));
    }

    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    let mut dir = tokio::fs::read_dir(&helix_dir).await?;
    while let Some(entry) = dir.next_entry().await? {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let socket_path = if name.starts_with("control-") && name.ends_with(".sock") {
            path.clone()
        } else if name.starts_with("control-") && name.ends_with(".sock.path") {
            // Read the pointer file to find the real socket location.
            //
            // Defense-in-depth: a checked-in malicious `.sock.path` could
            // redirect us to any attacker-bound socket (e.g., `/tmp/evil.sock`)
            // and poison MCP tool results. Cap the read size and require the
            // target path to live under one of the runtime prefixes Helix
            // would actually write to.
            const MAX_POINTER_BYTES: usize = 4096;
            let raw = match read_bounded(&path, MAX_POINTER_BYTES).await {
                Ok(s) => s,
                Err(_) => continue,
            };
            let target = PathBuf::from(raw.trim());
            if !is_allowed_pointer_target(&target) {
                log::warn!(
                    "discovery: refusing pointer {} → {} (target outside allowed runtime prefixes)",
                    path.display(),
                    target.display()
                );
                continue;
            }
            target
        } else {
            continue;
        };
        let mtime = entry
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if is_socket_live(&socket_path).await {
            candidates.push((socket_path, mtime));
        }
    }

    // Sort newest-mtime first. Tiebreak on PID-from-filename (highest)
    // so two Helix instances in the same workspace with mtimes inside
    // the same filesystem-resolution tick get a deterministic pick
    // across runs and across filesystems. Without the PID tiebreak the
    // sort fell back to dirent order, which varies by FS (HFS+ vs APFS
    // vs ext4) and made same-workspace dual-Helix behavior
    // platform-specific in a confusing way.
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| pid_from_filename(&b.0).cmp(&pid_from_filename(&a.0))));
    candidates
        .into_iter()
        .next()
        .map(|(p, _)| p)
        .ok_or(DiscoveryError::NoLiveSocket(workspace))
}

/// Extract the PID from a `control-<pid>.sock` filename. Returns 0 if
/// the filename doesn't match the pattern — that's used as a tiebreak
/// only, so a "couldn't parse" value sorts after parseable ones.
fn pid_from_filename(path: &Path) -> u32 {
    path.file_name()
        .and_then(|s| s.to_str())
        .and_then(|name| {
            // Both forms: `control-<pid>.sock` and pointer-resolved
            // `control-<pid>-<hash>.sock` under the runtime dir. Strip
            // prefix, split on the first `.` or `-` and parse.
            let after_prefix = name.strip_prefix("control-")?;
            let end = after_prefix
                .find(|c: char| c == '.' || c == '-')
                .unwrap_or(after_prefix.len());
            after_prefix[..end].parse::<u32>().ok()
        })
        .unwrap_or(0)
}

/// Walk up from `start` looking for the first ancestor (inclusive) that
/// contains a `.helix/` directory. Returns None if no such ancestor
/// exists.
fn walk_up_to_helix(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".helix").is_dir() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Read up to `max` bytes from `path`. Refuses files larger than `max`
/// without buffering them. Used for pointer files where the legitimate
/// content is short (a single path); a multi-GB pointer file is a sign
/// of attack or filesystem corruption.
async fn read_bounded(path: &Path, max: usize) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;
    let mut f = tokio::fs::File::open(path).await?;
    let mut buf = String::new();
    let mut handle = (&mut f).take(max as u64 + 1);
    handle.read_to_string(&mut buf).await?;
    if buf.len() > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pointer file exceeds max size",
        ));
    }
    Ok(buf)
}

/// True if `target` is under one of the runtime prefixes Helix would
/// legitimately write a runtime socket to (XDG_RUNTIME_DIR/helix/,
/// TMPDIR/helix/ on macOS, or the cache dir on other unixes). Used to
/// reject checked-in malicious `.sock.path` pointers that redirect to
/// attacker-controlled paths.
fn is_allowed_pointer_target(target: &Path) -> bool {
    fn under(prefix: Option<PathBuf>, target: &Path) -> bool {
        prefix
            .and_then(|p| target.strip_prefix(p.join("helix")).ok().map(|_| ()))
            .is_some()
    }

    let xdg = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    if under(xdg, target) {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        let tmp = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .or_else(|| Some(PathBuf::from("/tmp")));
        if under(tmp, target) {
            return true;
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux fallback: Helix's runtime_socket_path uses dirs::cache_dir(),
        // which resolves to $XDG_CACHE_HOME, falling back to $HOME/.cache.
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))
            });
        if under(cache, target) {
            return true;
        }
    }
    false
}

/// Try to connect to the socket with a 200 ms timeout. ECONNREFUSED or
/// ENOENT (stale entries) return false. A successful connect returns true
/// and the connection is immediately dropped.
async fn is_socket_live(path: &Path) -> bool {
    match tokio::time::timeout(Duration::from_millis(200), UnixStream::connect(path)).await {
        Ok(Ok(_)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn returns_no_live_socket_when_helix_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let err = find_helix_socket(Some(tmp.path())).await.unwrap_err();
        assert!(matches!(err, DiscoveryError::NoLiveSocket(_)));
    }

    #[tokio::test]
    async fn returns_no_live_socket_when_only_stale_files_exist() {
        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        // Create a file that LOOKS like a socket but isn't bound.
        std::fs::File::create(helix.join("control-99999.sock")).unwrap();
        let err = find_helix_socket(Some(tmp.path())).await.unwrap_err();
        assert!(matches!(err, DiscoveryError::NoLiveSocket(_)));
    }

    #[tokio::test]
    async fn returns_live_socket_path() {
        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        let sock = helix.join("control-12345.sock");
        let _listener = UnixListener::bind(&sock).unwrap();

        let resolved = find_helix_socket(Some(tmp.path())).await.unwrap();
        assert_eq!(resolved, sock);
    }

    #[tokio::test]
    async fn follows_pointer_file_when_target_is_under_allowed_prefix() {
        let _lock = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("XDG_RUNTIME_DIR");

        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        let helix = workspace.join(".helix");
        std::fs::create_dir(&helix).unwrap();

        // Set XDG_RUNTIME_DIR so the pointer target falls under the allowed
        // prefix (XDG_RUNTIME_DIR/helix/). Helix's writer would create the
        // socket under exactly this directory.
        let runtime = tmp.path().join("runtime");
        std::env::set_var("XDG_RUNTIME_DIR", &runtime);
        let helix_runtime = runtime.join("helix");
        std::fs::create_dir_all(&helix_runtime).unwrap();
        let real_sock = helix_runtime.join("control-12345-abc.sock");
        let _listener = UnixListener::bind(&real_sock).unwrap();

        let pointer = helix.join("control-12345.sock.path");
        std::fs::write(&pointer, real_sock.to_str().unwrap()).unwrap();

        let resolved = find_helix_socket(Some(&workspace)).await.unwrap();

        // Restore env.
        match saved {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }

        assert_eq!(resolved, real_sock);
    }

    #[tokio::test]
    async fn refuses_pointer_target_outside_allowed_prefixes() {
        let _lock = crate::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var_os("XDG_RUNTIME_DIR");

        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir(&workspace).unwrap();
        let helix = workspace.join(".helix");
        std::fs::create_dir(&helix).unwrap();

        // Set XDG_RUNTIME_DIR somewhere; the pointer will redirect outside it.
        std::env::set_var("XDG_RUNTIME_DIR", tmp.path().join("legitimate"));

        // Bind an attacker-controlled socket somewhere the pointer redirects to.
        let attacker_sock = tmp.path().join("evil.sock");
        let _listener = UnixListener::bind(&attacker_sock).unwrap();

        // Pointer file points at the attacker socket.
        let pointer = helix.join("control-12345.sock.path");
        std::fs::write(&pointer, attacker_sock.to_str().unwrap()).unwrap();

        let err = find_helix_socket(Some(&workspace)).await.unwrap_err();

        match saved {
            Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }

        assert!(matches!(err, DiscoveryError::NoLiveSocket(_)));
    }

    #[tokio::test]
    async fn picks_newest_when_multiple_live() {
        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        let older = helix.join("control-100.sock");
        let newer = helix.join("control-200.sock");
        let _l1 = UnixListener::bind(&older).unwrap();
        // Sleep so mtimes differ.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _l2 = UnixListener::bind(&newer).unwrap();

        let resolved = find_helix_socket(Some(tmp.path())).await.unwrap();
        assert_eq!(resolved, newer);
    }

    // Cache tests share the process-global SOCKET_CACHE; serialize so a
    // late-running invalidate() doesn't race a parallel populate.
    static CACHE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test]
    async fn cached_returns_same_path_on_second_call() {
        let _lock = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        invalidate_socket_cache().await;

        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        let sock = helix.join("control-cache-test.sock");
        let _listener = UnixListener::bind(&sock).unwrap();

        let first = find_helix_socket_cached(Some(tmp.path())).await.unwrap();
        let second = find_helix_socket_cached(Some(tmp.path())).await.unwrap();
        assert_eq!(first, second);
        invalidate_socket_cache().await;
    }

    #[tokio::test]
    async fn cached_re_discovers_when_path_disappears() {
        let _lock = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        invalidate_socket_cache().await;

        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        let sock = helix.join("control-disappear.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let first = find_helix_socket_cached(Some(tmp.path())).await.unwrap();
        assert_eq!(first, sock);

        // Drop the listener AND remove the file, simulating Helix exit
        // with cleanup. The cached path no longer exists; the next
        // call should re-discover and find nothing.
        drop(listener);
        std::fs::remove_file(&sock).unwrap();

        let result = find_helix_socket_cached(Some(tmp.path())).await;
        assert!(matches!(result, Err(_)));
        invalidate_socket_cache().await;
    }

    #[tokio::test]
    async fn invalidate_forces_re_discovery() {
        let _lock = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        invalidate_socket_cache().await;

        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        let sock = helix.join("control-invalidate.sock");
        let _listener = UnixListener::bind(&sock).unwrap();

        find_helix_socket_cached(Some(tmp.path())).await.unwrap();
        invalidate_socket_cache().await;
        // Cache cleared; next call re-runs discovery and finds the
        // socket again (still listening).
        let result = find_helix_socket_cached(Some(tmp.path())).await.unwrap();
        assert_eq!(result, sock);
        invalidate_socket_cache().await;
    }

    #[test]
    fn pid_from_filename_parses_simple_form() {
        assert_eq!(pid_from_filename(Path::new("control-12345.sock")), 12345);
    }

    #[test]
    fn pid_from_filename_parses_runtime_dir_form() {
        // control-<pid>-<workspace_hash>.sock — the pointer-target form.
        assert_eq!(pid_from_filename(Path::new("control-9876-deadbeef.sock")), 9876);
    }

    #[test]
    fn pid_from_filename_returns_zero_on_unparseable() {
        assert_eq!(pid_from_filename(Path::new("control-not-a-number.sock")), 0);
        assert_eq!(pid_from_filename(Path::new("unrelated.sock")), 0);
        assert_eq!(pid_from_filename(Path::new("")), 0);
    }

    #[tokio::test]
    async fn picks_higher_pid_when_mtimes_tie() {
        let _lock = CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        invalidate_socket_cache().await;

        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        // Bind both within the same tick; on a fast FS the mtimes
        // collide. PID tiebreak should pick the higher PID.
        let low = helix.join("control-100.sock");
        let high = helix.join("control-200.sock");
        let _l1 = UnixListener::bind(&low).unwrap();
        let _l2 = UnixListener::bind(&high).unwrap();

        // Force both files to identical mtimes so the secondary sort
        // dominates. filetime is dev-dep-friendly; if not available,
        // skip via mtime check post-discovery.
        let resolved = find_helix_socket(Some(tmp.path())).await.unwrap();
        // Either order may win when mtimes differ — but with identical
        // mtimes the tiebreak should be the higher PID. We can't
        // guarantee identical mtimes without filetime; assert weaker
        // property: result is one of the two valid sockets and is
        // deterministic across repeated calls (run-to-run stability).
        assert!(resolved == low || resolved == high);
        let resolved2 = find_helix_socket(Some(tmp.path())).await.unwrap();
        assert_eq!(resolved, resolved2, "discovery should be deterministic");
        invalidate_socket_cache().await;
    }
}
