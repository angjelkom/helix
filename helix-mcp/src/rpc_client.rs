//! JSON-RPC client that connects to Helix's control socket and exchanges
//! newline-delimited JSON-RPC messages. One request, one response per call;
//! does not reuse connections (Phase 4b may add connection pooling if
//! needed).

use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use helix_context_schema::{ControlRequest, ControlResponse, JsonRpcError};

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
pub async fn send_request(
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
}
