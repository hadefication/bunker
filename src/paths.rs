//! Where bunker keeps its files, and the one way it writes them (0600, atomically).
use crate::error::{BunkerError, Result};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// `$BUNKER_HOME`, else `~/.bunker`.
pub fn bunker_home() -> PathBuf {
    match std::env::var_os("BUNKER_HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".bunker"),
    }
}

/// `~/.bunker/<name>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDir {
    pub name: String,
    pub root: PathBuf,
}

impl ProjectDir {
    pub fn new(name: &str) -> Self {
        Self::under(&bunker_home(), name)
    }

    pub fn under(home: &Path, name: &str) -> Self {
        Self {
            name: name.to_string(),
            root: home.join(name),
        }
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("bunker.toml")
    }
    pub fn cloudflared_yml(&self) -> PathBuf {
        self.root.join("cloudflared.yml")
    }
    pub fn tunnel_credentials(&self) -> PathBuf {
        self.root.join("tunnel-credentials.json")
    }
    pub fn sandbox_sb(&self) -> PathBuf {
        self.root.join("sandbox.sb")
    }
    pub fn fwd_proxy_sb(&self) -> PathBuf {
        self.root.join("fwd-proxy.sb")
    }
    pub fn fwd_web_sb(&self) -> PathBuf {
        self.root.join("fwd-web.sb")
    }
    pub fn lima_yaml(&self) -> PathBuf {
        self.root.join("lima.yaml")
    }
    pub fn network_log(&self) -> PathBuf {
        self.root.join("network.jsonl")
    }
    pub fn leases_dir(&self) -> PathBuf {
        self.root.join("leases")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn deploy_lock(&self) -> PathBuf {
        self.root.join("deploy.lock")
    }
}

/// Creates `dir` (and parents) with mode 0700, and tightens it to 0700 if it already exists.
/// Creates `dir` (and parents) and makes it 0700, working through a descriptor opened with
/// `O_NOFOLLOW|O_DIRECTORY`: a symlink at `dir` itself is refused, and nothing can swap one in
/// between the check and the chmod.
pub fn ensure_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let refused = |why: &str| {
        BunkerError::new(
            "symlink_refused",
            crate::output::ExitCode::Refused,
            format!("{}: {why}; bunker won't follow it", dir.display()),
        )
    };
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| BunkerError::io(format!("cannot create {}", parent.display()), e))?;
    }
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => {
            return Err(BunkerError::io(
                format!("cannot create {}", dir.display()),
                e,
            ));
        }
    }
    let f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(dir)
        .map_err(|e| match e.raw_os_error() {
            Some(libc::ELOOP) => refused("it is a symlink"),
            Some(libc::ENOTDIR) => refused("it is not a directory"),
            _ => BunkerError::io(format!("cannot open {}", dir.display()), e),
        })?;
    let m = f
        .metadata()
        .map_err(|e| BunkerError::io(format!("cannot stat {}", dir.display()), e))?;
    // SAFETY: getuid has no preconditions.
    if !m.is_dir() || m.uid() != unsafe { libc::getuid() } {
        return Err(refused("it is not a directory this user owns"));
    }
    f.set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(|e| BunkerError::io(format!("cannot chmod {}", dir.display()), e))
}

/// Writes `bytes` to `path` with mode 0600: a temp file in the same directory, then a rename.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic(path, bytes, 0o600)
}

/// Writes `contents` to `path` with `mode`: a temp file in the same directory, then a rename, so a
/// reader (the proxy) never sees a half-written file.
pub fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        // Exact mode regardless of umask; never wider than asked while the file is being written.
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.write_all(contents)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        BunkerError::io(format!("cannot write {}", path.display()), e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_private_is_0600_and_replaces() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f.toml");
        write_private(&p, b"one").unwrap();
        write_private(&p, b"two").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "two");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::read_dir(d.path()).unwrap().count(),
            1,
            "no temp file left"
        );
    }

    #[test]
    fn write_atomic_keeps_mode_0600() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("bunker.toml");
        fs::write(&p, "old").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        write_atomic(&p, b"new", 0o600).unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), "new");
        write_atomic(&p, b"x", 0o664).unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o664,
            "umask must not apply"
        );
    }

    #[test]
    fn ensure_private_dir_refuses_a_symlink() {
        let d = tempfile::tempdir().unwrap();
        let victim = d.path().join("victim");
        fs::create_dir(&victim).unwrap();
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o755)).unwrap();
        let link = d.path().join("tmp");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        assert_eq!(
            ensure_private_dir(&link).unwrap_err().code,
            "symlink_refused"
        );
        assert_eq!(
            fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn ensure_private_dir_tightens_through_the_descriptor() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("a/b");
        ensure_private_dir(&dir).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::write(d.path().join("file"), "x").unwrap();
        assert!(
            ensure_private_dir(&d.path().join("file")).is_err(),
            "a file is not a directory"
        );
    }
}
