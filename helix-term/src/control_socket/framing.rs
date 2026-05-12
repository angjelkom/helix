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
    buf: String,
}

impl FrameReader {
    pub fn new(half: OwnedReadHalf) -> Self {
        Self {
            inner: BufReader::new(half),
            buf: String::new(),
        }
    }

    /// Read one JSON-RPC frame, parsed into ControlRequest. Returns None on
    /// EOF (peer closed). Errors on malformed JSON or oversize frame.
    pub async fn read_request(&mut self) -> io::Result<Option<ControlRequest>> {
        self.buf.clear();
        let n = self.inner.read_line(&mut self.buf).await?;
        if n == 0 {
            return Ok(None); // EOF
        }
        if self.buf.len() > MAX_FRAME_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
        }
        let trimmed = self.buf.trim_end_matches(['\r', '\n']);
        let req: ControlRequest = serde_json::from_str(trimmed)
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
