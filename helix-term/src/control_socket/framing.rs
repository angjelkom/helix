//! Newline-delimited JSON framing over async streams. One JSON object per
//! line, separated by a single `\n`. Lines longer than `MAX_FRAME_BYTES`
//! produce a framing error (defensive against malformed input).

use std::io;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use helix_context_schema::{ControlRequest, ControlResponse, JsonRpcError};

const MAX_FRAME_BYTES: usize = 1024 * 1024; // 1 MiB

pub struct FrameReader {
    inner: BufReader<OwnedReadHalf>,
}

impl FrameReader {
    pub fn new(half: OwnedReadHalf) -> Self {
        Self {
            inner: BufReader::new(half),
        }
    }

    /// Read one JSON-RPC frame, parsed into ControlRequest. Returns None on
    /// EOF (peer closed). Errors on malformed JSON or oversize frame.
    ///
    /// Uses an explicit fill_buf/consume loop so the size check trips
    /// before we've buffered more than `MAX_FRAME_BYTES + 1` — a peer that
    /// streams gigabytes without a newline cannot force unbounded buffer
    /// growth.
    pub async fn read_request(&mut self) -> io::Result<Option<ControlRequest>> {
        let mut bytes = Vec::with_capacity(256);
        loop {
            let chunk = self.inner.fill_buf().await?;
            if chunk.is_empty() {
                // EOF
                if bytes.is_empty() {
                    return Ok(None);
                }
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "frame ended without newline",
                ));
            }
            if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
                // Found end of frame within this chunk.
                bytes.extend_from_slice(&chunk[..pos]);
                let consume = pos + 1;
                self.inner.consume(consume);
                if bytes.len() > MAX_FRAME_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "frame too large",
                    ));
                }
                break;
            }
            // No newline in chunk; size check before appending.
            if bytes.len() + chunk.len() > MAX_FRAME_BYTES {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
            }
            bytes.extend_from_slice(chunk);
            let n = chunk.len();
            self.inner.consume(n);
        }
        // Convert to String, trimming CR if present (frames are LF-terminated
        // but tolerate CRLF for robustness with potential proxies).
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        let s = std::str::from_utf8(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let req: ControlRequest = serde_json::from_str(s)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(req))
    }
}

pub struct FrameWriter {
    inner: OwnedWriteHalf,
}

impl FrameWriter {
    pub fn new(half: OwnedWriteHalf) -> Self {
        Self { inner: half }
    }

    pub async fn write_response(
        &mut self,
        resp: &Result<ControlResponse, JsonRpcError>,
    ) -> io::Result<()> {
        let mut bytes = match resp {
            Ok(r) => serde_json::to_vec(r),
            Err(e) => serde_json::to_vec(e),
        }
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        bytes.push(b'\n');
        self.inner.write_all(&bytes).await?;
        self.inner.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helix_context_schema::ControlRequest;
    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn round_trip_initialize_request() {
        let (a, b) = UnixStream::pair().unwrap();
        let (a_read, _a_write) = a.into_split();
        let (_b_read, mut b_write) = b.into_split();

        let payload =
            br#"{"method":"initialize","params":{"protocol_version":"1.0","client_info":{"name":"test","version":"0.1"}}}"#;
        b_write.write_all(payload).await.unwrap();
        b_write.write_all(b"\n").await.unwrap();
        b_write.flush().await.unwrap();
        drop(b_write);

        let mut reader = FrameReader::new(a_read);
        let req = reader.read_request().await.unwrap().expect("expected a request");
        let ControlRequest::Initialize { protocol_version, client_info } = req else {
            panic!("expected Initialize variant");
        };
        assert_eq!(protocol_version, "1.0");
        assert_eq!(client_info.name, "test");

        let eof = reader.read_request().await.unwrap();
        assert!(eof.is_none());
    }

    #[tokio::test]
    async fn oversize_frame_without_newline_rejected_without_unbounded_buffering() {
        let (a, b) = UnixStream::pair().unwrap();
        let (a_read, _) = a.into_split();
        let (_, mut b_write) = b.into_split();

        // Drive the write from a background task so it can interleave with
        // the reader — Unix socket send buffers are well under 1 MiB on most
        // platforms, so a foreground `write_all(MAX+100)` would deadlock
        // before the reader gets a chance to drain.
        let writer = tokio::spawn(async move {
            let payload = vec![b'x'; MAX_FRAME_BYTES + 100];
            // Ignore the result — once the reader rejects + drops, the
            // peer write half breaks; the test cares about the reader's
            // error, not the writer's outcome.
            let _ = b_write.write_all(&payload).await;
            drop(b_write);
        });

        let mut reader = FrameReader::new(a_read);
        let err = reader.read_request().await.unwrap_err();
        let _ = writer.await;
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("frame too large"),
            "expected 'frame too large' error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn malformed_json_returns_invalid_data_error() {
        let (a, b) = UnixStream::pair().unwrap();
        let (a_read, _) = a.into_split();
        let (_, mut b_write) = b.into_split();

        b_write.write_all(b"not json at all\n").await.unwrap();
        b_write.flush().await.unwrap();

        let mut reader = FrameReader::new(a_read);
        let err = reader.read_request().await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
