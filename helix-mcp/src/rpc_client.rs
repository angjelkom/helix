//! JSON-RPC client that connects to Helix's control socket and exchanges
//! newline-delimited JSON-RPC messages. One request, one response per call;
//! does not reuse connections (Phase 4b may add connection pooling if
//! needed).

use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use helix_context_schema::{ControlRequest, ControlResponse, JsonRpcError};

const RECV_BUF_INITIAL: usize = 4096;

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
}
