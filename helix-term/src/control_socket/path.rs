//! Resolves the control socket path per spec §5.2.
//!
//! Priority:
//! 1. Explicit override from config
//! 2. `<workspace>/.helix/control-<pid>.sock` if its byte length fits in sun_path
//! 3. Runtime-dir fallback (with project-local pointer file)

use std::io;
use std::path::{Path, PathBuf};

const MAX_SUN_PATH: usize = 104;

#[derive(Debug)]
pub struct Resolved {
    pub primary: PathBuf,
    pub pointer_target: Option<PathBuf>,
}

pub fn resolve_socket_path(
    workspace: &Path,
    pid: u32,
    override_path: Option<&Path>,
) -> io::Result<Resolved> {
    if let Some(p) = override_path {
        return Ok(Resolved {
            primary: p.to_path_buf(),
            pointer_target: None,
        });
    }

    let project_local = workspace
        .join(".helix")
        .join(format!("control-{}.sock", pid));

    if project_local.as_os_str().len() <= MAX_SUN_PATH {
        return Ok(Resolved {
            primary: project_local,
            pointer_target: None,
        });
    }

    let runtime_socket = runtime_socket_path(workspace, pid)?;
    let pointer = workspace
        .join(".helix")
        .join(format!("control-{}.sock.path", pid));

    Ok(Resolved {
        primary: pointer,
        pointer_target: Some(runtime_socket),
    })
}

fn runtime_socket_path(workspace: &Path, pid: u32) -> io::Result<PathBuf> {
    let base = if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    } else {
        dirs::cache_dir().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no cache dir for runtime socket")
        })?
    };

    let workspace_hash = simple_hash(workspace.as_os_str().as_encoded_bytes());
    Ok(base
        .join("helix")
        .join(format!("control-{}-{:x}.sock", pid, workspace_hash)))
}

fn simple_hash(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn project_local_path_is_used_when_short_enough() {
        let workspace = PathBuf::from("/repo");
        let pid = 12345;
        let resolved = resolve_socket_path(&workspace, pid, None).unwrap();
        assert_eq!(resolved.primary, PathBuf::from("/repo/.helix/control-12345.sock"));
        assert!(resolved.pointer_target.is_none());
    }

    #[test]
    fn override_path_wins() {
        let workspace = PathBuf::from("/repo");
        let pid = 12345;
        let override_path = PathBuf::from("/custom/where.sock");
        let resolved = resolve_socket_path(&workspace, pid, Some(&override_path)).unwrap();
        assert_eq!(resolved.primary, override_path);
        assert!(resolved.pointer_target.is_none());
    }

    #[test]
    fn long_workspace_path_falls_back_to_runtime_dir() {
        let long = "/very/long/path".repeat(20);
        let workspace = PathBuf::from(&long);
        let pid = 12345;
        let resolved = resolve_socket_path(&workspace, pid, None).unwrap();
        assert!(resolved.primary.to_string_lossy().ends_with(".sock.path"));
        let target = resolved.pointer_target.expect("expected pointer target");
        assert!(target.to_string_lossy().contains("helix"));
        assert!(target.to_string_lossy().ends_with(".sock"));
        assert!(target.as_os_str().len() <= 104);
    }
}
