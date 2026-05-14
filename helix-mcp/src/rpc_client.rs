//! JSON-RPC client that connects to Helix's control socket and exchanges
//! newline-delimited JSON-RPC messages. One request, one response per call;
//! does not reuse connections (Phase 4b may add connection pooling if
//! needed).

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use helix_context_schema::{ClientInfo, ControlRequest, ControlResponse, JsonRpcError};

const RECV_BUF_INITIAL: usize = 4096;

/// Default overall timeout for a bridge → Helix RPC. Discovery has its
/// own 200 ms connect deadline; once connected, write+read have to land
/// within this window. 30 s is well past any LSP timeout (10 s on the
/// Helix side) so a stuck editor event loop — not a slow LSP — is what
/// trips it.
pub const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("connect to {0}: {1}")]
    Connect(std::path::PathBuf, std::io::Error),
    #[error("write request: {0}")]
    Write(std::io::Error),
    #[error("read response: {0}")]
    Read(std::io::Error),
    #[error("peer closed connection before sending a response")]
    PeerClosed,
    #[error("response was not valid JSON-RPC: {0}")]
    Parse(serde_json::Error),
    #[error("helix returned an error: {message} (code {code})", code = .0.code as i32, message = .0.message)]
    HelixError(JsonRpcError),
    #[error("timed out after {0:?} waiting for Helix to respond")]
    Timeout(Duration),
}

/// Connect to `socket_path`, send `request`, read one line of response,
/// parse it as either a successful `ControlResponse` or a `JsonRpcError`.
///
/// **Private to this module.** Phase 6b made `send_request_with_timeout`
/// mandatory in production — every production path now sets a deadline.
/// Tests in the same module still call this directly. If a future
/// caller genuinely wants "no timeout", pass `Duration::MAX` to the
/// public wrapper rather than reaching for this version.
async fn send_request(
    socket_path: &Path,
    request: &ControlRequest,
) -> Result<ControlResponse, RpcError> {
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|e| RpcError::Connect(socket_path.to_path_buf(), e))?;
    let (read_half, mut write_half) = stream.into_split();

    let mut payload = serde_json::to_vec(request)
        .map_err(|e| RpcError::Parse(e))?;
    payload.push(b'\n');
    write_half.write_all(&payload).await.map_err(RpcError::Write)?;
    write_half.flush().await.map_err(RpcError::Write)?;

    let mut reader = BufReader::with_capacity(RECV_BUF_INITIAL, read_half);
    let mut line = String::new();
    let n = reader.read_line(&mut line).await.map_err(RpcError::Read)?;
    if n == 0 {
        return Err(RpcError::PeerClosed);
    }

    // The line is either {"method":"...","result":{...}} or {"code":...,"message":...,...}
    // Try response first; if that fails, try error.
    if let Ok(resp) = serde_json::from_str::<ControlResponse>(line.trim()) {
        return Ok(resp);
    }
    let err: JsonRpcError = serde_json::from_str(line.trim())
        .map_err(RpcError::Parse)?;
    Err(RpcError::HelixError(err))
}

/// Same as `send_request` but bounded by `timeout`. Returns
/// `RpcError::Timeout` if the connect+write+read sequence doesn't
/// complete in time. Use this instead of `send_request` for any
/// production call where a hung Helix event loop is a possibility.
pub async fn send_request_with_timeout(
    socket_path: &Path,
    request: &ControlRequest,
    timeout: Duration,
) -> Result<ControlResponse, RpcError> {
    match tokio::time::timeout(timeout, send_request(socket_path, request)).await {
        Ok(r) => r,
        Err(_) => Err(RpcError::Timeout(timeout)),
    }
}

// ---------------------------------------------------------------------------
// Handshake cache
// ---------------------------------------------------------------------------
//
// Spec §6.1 mandates an `initialize` handshake with version negotiation
// before any other call. The bridge ran without it for the duration of
// Phase 4–6; today both ends are at protocol_version "1.0" so the gap was
// invisible. A future v2 bump on either side would surface as a confusing
// parse error rather than a clean version-mismatch refusal.
//
// `ensure_handshake` sends Initialize on first use and caches the
// outcome for the life of the process. Subsequent tool calls skip the
// round-trip. If a tool call later fails with a transport error
// (Helix restarted with a new PID and different protocol version, the
// process died, the socket vanished), dispatch_tool calls
// `invalidate_handshake_cache` so the next call re-handshakes.
//
// Per-process not per-connection: each `helix-mcp serve` is short-lived
// (Claude Code respawns it per session) and the bridge opens a fresh
// UnixStream per tool call, so caching at the process level saves one
// round-trip per call without staleness concerns in normal use. The
// invalidate-on-error path covers the rare cross-restart case.

/// Outcome of `ensure_handshake`. `Incompatible`'s fields are read by
/// `dispatch_tool` to compose the user-facing error message. `Ok`'s
/// fields are not currently consumed — they're captured here so a
/// future `serve --verbose` (Phase 7c §6.3) can log them on the first
/// successful handshake. The field-level `allow(dead_code)` keeps the
/// lint scoped: any *new* unused field on either variant gets flagged.
#[derive(Debug, Clone)]
pub enum HandshakeOutcome {
    Ok {
        #[allow(dead_code)]
        helix_version: String,
        #[allow(dead_code)]
        protocol_version: String,
    },
    Incompatible {
        helix_protocol: String,
        bridge_protocol: String,
    },
}

static HANDSHAKE_CACHE: OnceLock<tokio::sync::Mutex<Option<HandshakeOutcome>>> = OnceLock::new();

fn cache() -> &'static tokio::sync::Mutex<Option<HandshakeOutcome>> {
    HANDSHAKE_CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// Send `initialize` to Helix the first time this is called; subsequent
/// calls return the cached outcome. Returns an Incompatible outcome
/// (not Err) when the protocol-version majors disagree — callers turn
/// that into a friendly user-facing message rather than retrying.
///
/// Transport-level errors (connect/write/read/timeout) propagate as
/// `Err`; the cache is left empty so a future call re-attempts. Use
/// `invalidate_handshake_cache` to force a re-handshake on the next call
/// even if a prior call succeeded — typically called from dispatch_tool
/// after a transport-level error during the actual tool RPC.
pub async fn ensure_handshake(socket_path: &Path) -> Result<HandshakeOutcome, RpcError> {
    let mut guard = cache().lock().await;
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
    let resp = send_request_with_timeout(socket_path, &req, Duration::from_secs(5)).await?;
    let outcome = match resp {
        ControlResponse::Initialize {
            protocol_version,
            helix_version,
            ..
        } => {
            if is_compatible_major(&protocol_version, helix_context_schema::PROTOCOL_VERSION) {
                HandshakeOutcome::Ok {
                    helix_version,
                    protocol_version,
                }
            } else {
                HandshakeOutcome::Incompatible {
                    helix_protocol: protocol_version,
                    bridge_protocol: helix_context_schema::PROTOCOL_VERSION.into(),
                }
            }
        }
        // Helix returned a response, but not the Initialize variant. Treat
        // as a transport-level failure — the bridge can't trust the wire
        // shape from this peer.
        other => {
            return Err(RpcError::Parse(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Helix returned a non-Initialize response to `initialize`: {:?}",
                    std::mem::discriminant(&other)
                ),
            ))));
        }
    };
    *guard = Some(outcome.clone());
    Ok(outcome)
}

/// Clear the cached handshake outcome. The next `ensure_handshake` will
/// re-run the handshake. dispatch_tool calls this on transport errors
/// during a tool RPC, since those usually mean Helix went away and may
/// have come back with a different protocol_version.
pub async fn invalidate_handshake_cache() {
    let mut guard = cache().lock().await;
    *guard = None;
}

fn is_compatible_major(a: &str, b: &str) -> bool {
    let major = |s: &str| s.split('.').next().and_then(|n| n.parse::<u32>().ok());
    matches!((major(a), major(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_context_schema::{ClientInfo, ControlResponse, PROTOCOL_VERSION};
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    /// Spawn a fake Helix-side that accepts one connection, reads one line,
    /// responds with a canned response, then exits.
    async fn spawn_fake_helix(
        socket_path: std::path::PathBuf,
        response_line: String,
    ) -> tokio::task::JoinHandle<()> {
        let listener = UnixListener::bind(&socket_path).unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            // read just enough — we don't validate; we send the canned response.
            use tokio::io::AsyncReadExt;
            let _ = stream.read(&mut buf).await.unwrap();
            use tokio::io::AsyncWriteExt;
            stream.write_all(response_line.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();
        })
    }

    #[tokio::test]
    async fn send_request_parses_success_response() {
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("fake.sock");
        let canned = r#"{"method":"initialize","result":{"protocol_version":"1.0","helix_version":"25.7.1","server_info":{"name":"helix","version":"25.7.1"},"capabilities":{"read_methods":[],"write_methods":[]}}}"#;
        let _server = spawn_fake_helix(sock.clone(), format!("{}\n", canned)).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let req = ControlRequest::Initialize {
            protocol_version: PROTOCOL_VERSION.into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let resp = send_request(&sock, &req).await.unwrap();
        let ControlResponse::Initialize { protocol_version, .. } = resp else {
            panic!("wrong variant");
        };
        assert_eq!(protocol_version, "1.0");
    }

    #[tokio::test]
    async fn send_request_parses_error_response() {
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("fake-err.sock");
        let canned = r#"{"code":-32601,"message":"Method 'foo' not found"}"#;
        let _server = spawn_fake_helix(sock.clone(), format!("{}\n", canned)).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let req = ControlRequest::Initialize {
            protocol_version: PROTOCOL_VERSION.into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let err = send_request(&sock, &req).await.unwrap_err();
        match err {
            RpcError::HelixError(je) => assert_eq!(je.message, "Method 'foo' not found"),
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[tokio::test]
    async fn send_request_returns_connect_error_for_missing_socket() {
        let nonexistent = std::path::PathBuf::from("/tmp/definitely-not-here.sock");
        let req = ControlRequest::Initialize {
            protocol_version: PROTOCOL_VERSION.into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let err = send_request(&nonexistent, &req).await.unwrap_err();
        assert!(matches!(err, RpcError::Connect(_, _)), "got: {:?}", err);
    }

    #[tokio::test]
    async fn send_request_with_timeout_returns_timeout_when_helix_never_responds() {
        // Bind a listener but never write a response. send_request_with_timeout
        // should trip on its own deadline rather than waiting indefinitely.
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("hanging.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        // Accept the connection and keep the stream alive without writing.
        // The `accept_task` is aborted at the end of the test.
        let accept_task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            // Drain stdin so the bridge's write_all doesn't get backpressured
            // off the kernel send buffer, then sleep indefinitely.
            use tokio::io::AsyncReadExt;
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        });

        let req = ControlRequest::Initialize {
            protocol_version: PROTOCOL_VERSION.into(),
            client_info: ClientInfo { name: "t".into(), version: "0.1".into() },
        };
        let start = std::time::Instant::now();
        let result = send_request_with_timeout(
            &sock,
            &req,
            Duration::from_millis(300),
        )
        .await;
        let elapsed = start.elapsed();

        accept_task.abort();
        assert!(matches!(result, Err(RpcError::Timeout(_))), "got: {:?}", result);
        // Soft upper bound — should fire within a comfortable margin of the
        // 300ms deadline, not creep up toward the 60s sleep.
        assert!(
            elapsed < Duration::from_secs(2),
            "timeout took too long: {:?}",
            elapsed
        );
    }

    // ----- Handshake tests --------------------------------------------------
    //
    // These tests share the process-global HANDSHAKE_CACHE. They serialize
    // against each other via HANDSHAKE_CACHE_LOCK so an invalidation in one
    // test doesn't race against a populate in another. Each test also
    // invalidates the cache on entry to start from a known state.

    static HANDSHAKE_CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Same as spawn_fake_helix but the fake recognizes `initialize` and
    /// returns a valid response; anything else gets `canned`. Mirrors the
    /// integration test fixture so handshake-aware tests don't need to
    /// hand-roll branching.
    async fn spawn_fake_helix_with_handshake(
        socket_path: std::path::PathBuf,
        canned: String,
    ) -> tokio::task::JoinHandle<()> {
        let listener = UnixListener::bind(&socket_path).unwrap();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((mut stream, _)) => {
                        let canned = canned.clone();
                        tokio::spawn(async move {
                            use tokio::io::{AsyncReadExt, AsyncWriteExt};
                            let mut buf = vec![0u8; 4096];
                            let n = stream.read(&mut buf).await.unwrap_or(0);
                            let req = std::str::from_utf8(&buf[..n]).unwrap_or("");
                            let resp = if req.contains("\"method\":\"initialize\"") {
                                r#"{"method":"initialize","result":{"protocol_version":"1.0","helix_version":"t","server_info":{"name":"f","version":"0"},"capabilities":{"read_methods":[],"write_methods":[]}}}
"#
                                .to_string()
                            } else {
                                canned
                            };
                            let _ = stream.write_all(resp.as_bytes()).await;
                            let _ = stream.flush().await;
                        });
                    }
                    Err(_) => break,
                }
            }
        })
    }

    #[tokio::test]
    async fn ensure_handshake_caches_outcome_after_first_call() {
        let _lock = HANDSHAKE_CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        invalidate_handshake_cache().await;

        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("cache.sock");
        let task = spawn_fake_helix_with_handshake(sock.clone(), "irrelevant".into()).await;

        // First call: should reach the fake and populate the cache.
        let first = ensure_handshake(&sock).await.unwrap();
        assert!(matches!(first, HandshakeOutcome::Ok { .. }));

        // Abort the fake so a second handshake would fail at connect.
        // If the cache is honored, the second call returns the cached
        // outcome without touching the socket.
        task.abort();
        let second = ensure_handshake(&sock).await.unwrap();
        assert!(matches!(second, HandshakeOutcome::Ok { .. }));

        invalidate_handshake_cache().await;
    }

    #[tokio::test]
    async fn invalidate_handshake_cache_forces_re_handshake() {
        let _lock = HANDSHAKE_CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        invalidate_handshake_cache().await;

        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("invalidate.sock");
        let task = spawn_fake_helix_with_handshake(sock.clone(), "x".into()).await;
        ensure_handshake(&sock).await.unwrap();

        // Tear down the fake — without invalidation, the cached outcome
        // would still satisfy `ensure_handshake` even with no listener.
        task.abort();
        // Wait for the listener task to fully release the bound socket
        // path so the next ensure_handshake's connect actually fails
        // rather than racing with a half-closed listener.
        tokio::task::yield_now().await;
        std::fs::remove_file(&sock).ok();

        invalidate_handshake_cache().await;

        let result = ensure_handshake(&sock).await;
        assert!(matches!(result, Err(_)), "got: {:?}", result);
    }

    #[tokio::test]
    async fn ensure_handshake_does_not_cache_on_transport_error() {
        let _lock = HANDSHAKE_CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        invalidate_handshake_cache().await;

        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("nonexistent.sock");
        // No listener at this path.
        let result = ensure_handshake(&sock).await;
        assert!(matches!(result, Err(_)));

        // A subsequent call should also error (it would never silently
        // succeed from a cache) — proving the failed handshake didn't
        // poison the cache.
        let result2 = ensure_handshake(&sock).await;
        assert!(matches!(result2, Err(_)));
    }
}
