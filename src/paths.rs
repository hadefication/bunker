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
pub fn ensure_private_dir(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| BunkerError::io(format!("cannot create {}", dir.display()), e))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| BunkerError::io(format!("cannot chmod {}", dir.display()), e))
}

/// Writes `bytes` to `path` with mode 0600: a temp file in the same directory, then a rename.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let io = |e| BunkerError::io(format!("cannot write {}", path.display()), e);
    let write = || -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io(e)
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
}
