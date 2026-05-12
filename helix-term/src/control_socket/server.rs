//! Spawns the per-connection tasks that read requests, dispatch them, and
//! write responses. The outer accept loop runs as a tokio task spawned by
//! `Application::start_control_server`.

use tokio::net::UnixListener;
use tokio::sync::mpsc::Sender;

use helix_view::editor::EditorEvent;

use crate::control_socket::dispatch::try_dispatch_inline;
use crate::control_socket::framing::{FrameReader, FrameWriter};

/// Accept connections forever and spawn a per-connection task for each.
/// Returns when the listener is dropped (which happens on Application::close).
pub async fn run_accept_loop(listener: UnixListener, control_tx: Sender<EditorEvent>) {
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

async fn handle_connection(
    stream: tokio::net::UnixStream,
    control_tx: Sender<EditorEvent>,
) {
    let (read_half, write_half) = stream.into_split();
    let mut reader = FrameReader::new(read_half);
    let mut writer = FrameWriter::new(write_half);

    loop {
        let req = match reader.read_request().await {
            Ok(Some(req)) => req,
            Ok(None) => break,
            Err(e) => {
                log::warn!("control-socket: frame read error: {}", e);
                let _ = writer
                    .write_response(&Err(
                        helix_context_schema::JsonRpcError {
                            code: helix_context_schema::JsonRpcErrorCode::ParseError,
                            message: format!("{}", e),
                            data: None,
                        },
                    ))
                    .await;
                break;
            }
        };

        let resp = match try_dispatch_inline(&req) {
            Some(resp) => resp,
            None => {
                // Forward into the main event loop via the channel.
                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                let event = helix_view::editor::EditorEvent::ControlRequest {
                    request: req,
                    reply: reply_tx,
                };
                if control_tx.send(event).await.is_err() {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_socket::lifecycle::{bind_socket, unlink};
    use crate::control_socket::path::Resolved;
    use tempfile::TempDir;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn client_can_complete_initialize_handshake() {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("ipc.sock");
        let resolved = Resolved {
            primary: socket_path.clone(),
            pointer_target: None,
        };
        let binding = bind_socket(resolved).unwrap();

        let (control_tx, _control_rx) = tokio::sync::mpsc::channel::<helix_view::editor::EditorEvent>(64);
        let server = tokio::spawn(run_accept_loop(binding.listener, control_tx));

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let request =
            br#"{"method":"initialize","params":{"protocol_version":"1.0","client_info":{"name":"test","version":"0.1"}}}"#;
        client.write_all(request).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.flush().await.unwrap();

        let (read_half, _) = client.into_split();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains(r#""method":"initialize""#));
        assert!(line.contains(r#""protocol_version":"1.0""#));
        assert!(line.contains(r#""helix_version""#));

        server.abort();
        unlink(&Resolved { primary: socket_path, pointer_target: None }).ok();
    }
}
