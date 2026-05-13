//! Find the right Helix control socket for the current workspace.
//!
//! Per spec §7.4, discovery globs `<workspace>/.helix/control-*.sock` plus
//! any pointer files (`*.sock.path` — used when the project-local path
//! would exceed sun_path). Filters out unconnectable sockets via a brief
//! connect attempt. If multiple live sockets exist, picks the one with the
//! newest mtime.

use std::path::{Path, PathBuf};
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

/// Discover the live Helix control socket. Returns the path that should be
/// passed to `rpc_client::send_request`.
///
/// `workspace_override` lets callers (and tests) skip env-var lookup.
pub async fn find_helix_socket(
    workspace_override: Option<&Path>,
) -> Result<PathBuf, DiscoveryError> {
    let workspace = match workspace_override {
        Some(p) => p.to_path_buf(),
        None => {
            std::env::var_os("CLAUDE_PROJECT_DIR")
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
                .ok_or(DiscoveryError::NoWorkspace)?
        }
    };

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
            match tokio::fs::read_to_string(&path).await {
                Ok(contents) => PathBuf::from(contents.trim()),
                Err(_) => continue,
            }
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

    candidates.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
    candidates
        .into_iter()
        .next()
        .map(|(p, _)| p)
        .ok_or(DiscoveryError::NoLiveSocket(workspace))
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
    async fn follows_pointer_file() {
        let tmp = TempDir::new().unwrap();
        let helix = tmp.path().join(".helix");
        std::fs::create_dir(&helix).unwrap();
        // Create the real socket somewhere outside the .helix dir.
        let real_sock = tmp.path().join("real.sock");
        let _listener = UnixListener::bind(&real_sock).unwrap();
        // Pointer file at expected location.
        let pointer = helix.join("control-12345.sock.path");
        std::fs::write(&pointer, real_sock.to_str().unwrap()).unwrap();

        let resolved = find_helix_socket(Some(tmp.path())).await.unwrap();
        assert_eq!(resolved, real_sock);
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
}
