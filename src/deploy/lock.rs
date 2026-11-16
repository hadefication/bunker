//! One deploy at a time per project: an flock on `~/.bunker/<project>/deploy.lock`.
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use std::path::Path;

/// Held for the deploy; the kernel releases it when the process exits, however it exits.
#[derive(Debug)]
pub struct DeployLock {
    _file: std::fs::File,
}

/// Takes the lock without waiting; a deploy already holding it is exit 4 `deploy_in_progress`.
pub fn acquire(path: &Path) -> Result<DeployLock> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| BunkerError::io(format!("cannot open {}", path.display()), e))?;
    // SAFETY: flock on a descriptor this function owns.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(BunkerError::new(
                "deploy_in_progress",
                ExitCode::Refused,
                "another deploy or rollback of this project is running",
            ));
        }
        return Err(BunkerError::io(
            format!("cannot lock {}", path.display()),
            err,
        ));
    }
    Ok(DeployLock { _file: file })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_deploy_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("deploy.lock");
        let held = acquire(&p).unwrap();
        let e = acquire(&p).unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("deploy_in_progress", crate::output::ExitCode::Refused)
        );
        drop(held);
        acquire(&p).unwrap();
    }
}
