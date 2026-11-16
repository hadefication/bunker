//! Time-boxed openings of the build and maintenance lists: a file holding its expiry. A crashed
//! deploy can't leave the build list open past the TTL, and dropping the guard closes it early.
use crate::error::Result;
use crate::net::log::{EventKind, NetEvent, append_event};
use crate::paths::{ProjectDir, ensure_private_dir, write_private};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseKind {
    Build,
    Maintenance,
}

impl LeaseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseKind::Build => "build",
            LeaseKind::Maintenance => "maintenance",
        }
    }
}

#[derive(Debug)]
pub struct LeaseGuard {
    dir: ProjectDir,
    kind: LeaseKind,
}

fn lease_file(dir: &ProjectDir, kind: LeaseKind) -> std::path::PathBuf {
    dir.leases_dir().join(kind.as_str())
}

fn log(dir: &ProjectDir, kind: LeaseKind, action: &str, ttl_secs: Option<u64>) -> Result<()> {
    append_event(
        &dir.network_log(),
        &NetEvent::now(
            &dir.name,
            EventKind::Lease {
                lease_kind: kind.as_str().into(),
                action: action.into(),
                ttl_secs,
            },
        ),
    )
}

/// Writes the expiry (unix seconds) to `leases/<kind>` and logs `lease open`.
pub fn open_lease(dir: &ProjectDir, kind: LeaseKind, ttl: Duration) -> Result<LeaseGuard> {
    ensure_private_dir(&dir.leases_dir())?;
    let expiry = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        + ttl;
    write_private(
        &lease_file(dir, kind),
        expiry.as_secs().to_string().as_bytes(),
    )?;
    let guard = LeaseGuard {
        dir: dir.clone(),
        kind,
    };
    log(dir, kind, "open", Some(ttl.as_secs()))?;
    Ok(guard)
}

/// False for a missing, expired or unreadable lease file.
pub fn lease_active(dir: &ProjectDir, kind: LeaseKind, now: SystemTime) -> bool {
    let Ok(text) = std::fs::read_to_string(lease_file(dir, kind)) else {
        return false;
    };
    let Ok(expiry) = text.trim().parse::<u64>() else {
        return false;
    };
    now.duration_since(UNIX_EPOCH)
        .is_ok_and(|n| n.as_secs() < expiry)
}

impl Drop for LeaseGuard {
    /// Removes the file and logs `lease close`. Errors are ignored: the expiry still bounds it.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(lease_file(&self.dir, self.kind));
        let _ = log(&self.dir, self.kind, "close", None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::log::{EventKind, read_events};

    fn project() -> (tempfile::TempDir, ProjectDir) {
        let home = tempfile::tempdir().unwrap();
        let pd = ProjectDir::under(home.path(), "p");
        std::fs::create_dir_all(&pd.root).unwrap();
        (home, pd)
    }

    #[test]
    fn expired_lease_is_inactive() {
        let (_h, pd) = project();
        let g = open_lease(&pd, LeaseKind::Build, Duration::from_secs(60)).unwrap();
        let now = SystemTime::now();
        assert!(lease_active(&pd, LeaseKind::Build, now));
        assert!(!lease_active(&pd, LeaseKind::Maintenance, now));
        assert!(!lease_active(
            &pd,
            LeaseKind::Build,
            now + Duration::from_secs(61)
        ));
        std::mem::forget(g);
    }

    #[test]
    fn lease_guard_removes_file_on_drop() {
        let (_h, pd) = project();
        let g = open_lease(&pd, LeaseKind::Maintenance, Duration::from_secs(60)).unwrap();
        let file = pd.leases_dir().join("maintenance");
        assert!(file.exists());
        drop(g);
        assert!(!file.exists());
        assert!(!lease_active(
            &pd,
            LeaseKind::Maintenance,
            SystemTime::now()
        ));
        let actions: Vec<String> = read_events(&pd.network_log())
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.kind {
                EventKind::Lease {
                    action,
                    lease_kind,
                    ttl_secs,
                } => Some(format!("{lease_kind} {action} {ttl_secs:?}")),
                _ => None,
            })
            .collect();
        assert_eq!(
            actions,
            ["maintenance open Some(60)", "maintenance close None"]
        );
    }

    #[test]
    fn garbage_lease_file_is_inactive() {
        let (_h, pd) = project();
        std::fs::create_dir_all(pd.leases_dir()).unwrap();
        std::fs::write(pd.leases_dir().join("build"), "soon").unwrap();
        assert!(!lease_active(&pd, LeaseKind::Build, SystemTime::now()));
    }
}
