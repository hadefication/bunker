//! Is this VM's hostagent the sandboxed one bunker started? `ha.pid` names the hostagent, and
//! `sandbox_check` asks the kernel whether that process runs under a Seatbelt profile.
use crate::error::Result;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confinement {
    Confined { hostagent: i32 },
    Unconfined { hostagent: i32, reason: String },
    Stopped,
}

impl Confinement {
    pub fn as_str(&self) -> &'static str {
        match self {
            Confinement::Confined { .. } => "confined",
            Confinement::Unconfined { .. } => "unconfined",
            Confinement::Stopped => "stopped",
        }
    }
}

pub trait ConfinementProbe: Send + Sync {
    fn confinement(&self, project: &str) -> Result<Confinement>;
}

pub struct SystemProbe {
    pub lima_home: PathBuf,
}

/// True only when the kernel says `pid` is sandboxed (1); not sandboxed (0) and errors (-1) are false.
pub fn pid_sandboxed(pid: i32) -> bool {
    unsafe extern "C" {
        // libsandbox (in libSystem): int sandbox_check(pid_t, const char *operation, int type, ...)
        fn sandbox_check(
            pid: libc::pid_t,
            operation: *const libc::c_char,
            filter: libc::c_int,
            ...
        ) -> libc::c_int;
    }
    // SAFETY: a NULL operation with SANDBOX_FILTER_NONE (0) only asks whether `pid` is sandboxed.
    unsafe { sandbox_check(pid, std::ptr::null(), 0) == 1 }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 checks existence and permission only.
    pid > 0 && unsafe { libc::kill(pid, 0) == 0 }
}

impl ConfinementProbe for SystemProbe {
    /// Stopped when there is no live hostagent; otherwise confined only if the kernel says its
    /// process is sandboxed. A manual `limactl start` produces an unsandboxed hostagent.
    fn confinement(&self, project: &str) -> Result<Confinement> {
        let ha = self
            .lima_home
            .join(crate::lima::vm::vm_name(project))
            .join("ha.pid");
        let Some(pid) = std::fs::read_to_string(&ha)
            .ok()
            .and_then(|t| t.trim().parse::<i32>().ok())
        else {
            return Ok(Confinement::Stopped);
        };
        if !alive(pid) {
            return Ok(Confinement::Stopped);
        }
        Ok(if pid_sandboxed(pid) {
            Confinement::Confined { hostagent: pid }
        } else {
            Confinement::Unconfined {
                hostagent: pid,
                reason: format!(
                    "hostagent {pid} is not sandboxed; the VM was started outside bunker (stop it with limactl stop, then bunker start)"
                ),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[test]
    fn missing_ha_pid_is_stopped() {
        let d = tempfile::tempdir().unwrap();
        let p = SystemProbe {
            lima_home: d.path().to_path_buf(),
        };
        assert_eq!(p.confinement("t").unwrap(), Confinement::Stopped);
        std::fs::create_dir_all(d.path().join("bunker-t")).unwrap();
        std::fs::write(d.path().join("bunker-t/ha.pid"), "999999\n").unwrap();
        assert_eq!(
            p.confinement("t").unwrap(),
            Confinement::Stopped,
            "dead pid"
        );
    }

    #[test]
    #[ignore]
    fn sys_confinement_detects_unsandboxed_pid() {
        let mut plain = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut boxed = std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/bin/sleep", "30"])
            .spawn()
            .unwrap();
        let (pp, bp) = (plain.id() as i32, boxed.id() as i32);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(alive(pp) && alive(bp));
        assert!(!pid_sandboxed(pp), "plain sleep reported sandboxed");
        assert!(
            pid_sandboxed(bp),
            "sandbox-exec'd sleep reported unsandboxed"
        );

        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("bunker-t")).unwrap();
        let probe = SystemProbe {
            lima_home: d.path().to_path_buf(),
        };
        std::fs::write(d.path().join("bunker-t/ha.pid"), format!("{pp}\n")).unwrap();
        assert!(
            matches!(probe.confinement("t").unwrap(), Confinement::Unconfined { hostagent, .. } if hostagent == pp)
        );
        std::fs::write(d.path().join("bunker-t/ha.pid"), format!("{bp}\n")).unwrap();
        assert_eq!(
            probe.confinement("t").unwrap(),
            Confinement::Confined { hostagent: bp }
        );
        for c in [&mut plain, &mut boxed] {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
