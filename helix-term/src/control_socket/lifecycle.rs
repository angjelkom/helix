//! Socket bind + lifecycle per spec §5.3.

use std::io;
use std::path::Path;
use tokio::net::UnixListener;

use crate::control_socket::path::Resolved;

pub struct Binding {
    pub listener: UnixListener,
    pub resolved: Resolved,
}

/// Bind the control socket. Handles orphan cleanup, umask for atomic 0600
/// mode, and writes the pointer file if the path resolution required one.
pub fn bind_socket(resolved: Resolved) -> io::Result<Binding> {
    let bind_path: &Path = resolved
        .pointer_target
        .as_deref()
        .unwrap_or(&resolved.primary);

    if let Some(parent) = bind_path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            let _ = std::fs::set_permissions(parent, perms);
        }
    }

    if bind_path.exists() {
        if is_socket_live(bind_path) {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "control socket {} is already owned by a live process",
                    bind_path.display()
                ),
            ));
        }
        std::fs::remove_file(bind_path)?;
    }

    let listener = with_strict_umask(|| UnixListener::bind(bind_path))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(bind_path, perms)?;
    }

    if resolved.pointer_target.is_some() {
        let primary = &resolved.primary;
        if let Some(parent) = primary.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(primary, bind_path.to_string_lossy().as_bytes())?;
    }

    Ok(Binding { listener, resolved })
}

/// Unlink everything bind_socket created (socket file + optional pointer
/// file). Called from Application::close.
pub fn unlink(resolved: &Resolved) -> io::Result<()> {
    let bind_path: &Path = resolved
        .pointer_target
        .as_deref()
        .unwrap_or(&resolved.primary);
    if bind_path.exists() {
        std::fs::remove_file(bind_path)?;
    }
    if resolved.pointer_target.is_some() && resolved.primary.exists() {
        std::fs::remove_file(&resolved.primary)?;
    }
    Ok(())
}

fn is_socket_live(path: &Path) -> bool {
    use std::os::unix::net::UnixStream;
    match UnixStream::connect(path) {
        Ok(_) => true,
        Err(_) => false,
    }
}

fn with_strict_umask<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    #[cfg(unix)]
    {
        let prev = unsafe { libc::umask(0o077) };
        let out = f();
        unsafe {
            libc::umask(prev);
        }
        out
    }
    #[cfg(not(unix))]
    {
        f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn bind_then_unlink_leaves_no_files() {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");
        let resolved = Resolved {
            primary: socket_path.clone(),
            pointer_target: None,
        };
        let binding = bind_socket(resolved).unwrap();
        assert!(socket_path.exists(), "socket file should exist after bind");
        drop(binding.listener);
        unlink(&Resolved { primary: socket_path.clone(), pointer_target: None }).unwrap();
        assert!(!socket_path.exists(), "socket file should be gone after unlink");
    }

    #[tokio::test]
    async fn bind_unlinks_existing_orphan_socket_file() {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("orphan.sock");
        std::fs::File::create(&socket_path).unwrap();
        let resolved = Resolved {
            primary: socket_path.clone(),
            pointer_target: None,
        };
        let binding = bind_socket(resolved).unwrap();
        assert!(socket_path.exists(), "new socket should be bound at the same path");
        drop(binding.listener);
        unlink(&Resolved { primary: socket_path, pointer_target: None }).ok();
    }

    #[tokio::test]
    async fn pointer_file_is_written_when_resolved_has_target() {
        let tmp = TempDir::new().unwrap();
        let pointer = tmp.path().join("pointer.sock.path");
        let actual = tmp.path().join("real.sock");
        let resolved = Resolved {
            primary: pointer.clone(),
            pointer_target: Some(actual.clone()),
        };
        let binding = bind_socket(resolved).unwrap();
        assert!(pointer.exists(), "pointer file should exist");
        assert!(actual.exists(), "real socket file should exist");
        let pointer_contents = std::fs::read_to_string(&pointer).unwrap();
        assert_eq!(pointer_contents.trim(), actual.to_string_lossy());
        drop(binding.listener);
        unlink(&Resolved { primary: pointer.clone(), pointer_target: Some(actual.clone()) }).ok();
        assert!(!pointer.exists());
        assert!(!actual.exists());
    }
}
