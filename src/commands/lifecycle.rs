//! `bunker start|stop|restart <project>`.
use crate::config::Config;
use crate::error::{BunkerError, Result};
use crate::host::Host;
use crate::launchd::{Role, ensure_in, label, remove_in};
use crate::lima::vm::{agents_for, instance_status, lima_vm_dir, vm_name, wait_running};
use crate::output::{ExitCode, Report};
use crate::paths::ProjectDir;
use crate::run::Cmd;
use serde_json::{Value, json};
use std::io::{self, Write};

#[derive(Debug)]
pub struct LifecycleReport {
    pub project: String,
    pub action: &'static str,
    pub confinement: String,
}

impl Report for LifecycleReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "{}: {} ({})",
            self.project, self.action, self.confinement
        )
    }
    fn json(&self) -> Value {
        json!({"project": self.project, "action": self.action, "confinement": self.confinement})
    }
    fn exit(&self) -> crate::output::ExitCode {
        if self.confinement == "unconfined" {
            crate::output::ExitCode::Unconfined
        } else {
            crate::output::ExitCode::Ok
        }
    }
}

/// Installs the vm agent (the only way bunker starts a VM), waits for it, then the forwards.
pub fn start(h: &Host, project: &str) -> Result<LifecycleReport> {
    let pd = ProjectDir::under(&h.home, project);
    let cfg = Config::load(&pd)?;
    let vm = vm_name(project);
    if instance_status(h, &vm)?.is_none() {
        return Err(BunkerError::new(
            "vm_not_found",
            ExitCode::NotFound,
            format!("{vm} does not exist"),
        )
        .hint(format!("run bunker up {project}")));
    }
    let vm_dir = lima_vm_dir(&h.lima_home, &vm)?;
    crate::lima::vm::check_sealed(&vm_dir)?;
    let [vm_agent, fwd_proxy, fwd_web] = agents_for(project, &cfg, &pd, &h.limactl, &vm_dir);
    ensure_in(h.r, &h.agents_dir, &vm_agent)?;
    wait_running(h, &vm)?;
    ensure_in(h.r, &h.agents_dir, &fwd_proxy)?;
    ensure_in(h.r, &h.agents_dir, &fwd_web)?;
    let c = h.probe.confinement(project)?;
    crate::commands::up::sync_tunnel(h, &pd, &cfg, &c)?;
    Ok(LifecycleReport {
        project: project.to_string(),
        action: "started",
        confinement: c.as_str().to_string(),
    })
}

/// Removes the tunnel (off the internet first), the forwards, then the vm agent (plists too, so
/// nothing returns at login), then stops the VM if the hostagent's exit didn't already.
pub fn stop(h: &Host, project: &str) -> Result<LifecycleReport> {
    for role in [Role::Tunnel, Role::FwdProxy, Role::FwdWeb, Role::Vm] {
        remove_in(h.r, &h.agents_dir, &label(Some(project), role))?;
    }
    let vm = vm_name(project);
    // Booting out the vm agent sends the hostagent SIGTERM, and it stops the VM itself. Calling
    // limactl stop while it is mid-shutdown waits minutes for an event that never comes, so wait
    // for that first and only stop a VM that is still running afterwards.
    let deadline = std::time::Instant::now() + h.wait_timeout;
    while instance_status(h, &vm)?.as_deref() == Some("Running") {
        if std::time::Instant::now() >= deadline {
            h.r.run_ok(&Cmd::new(h.limactl()).args(["stop", "--tty=false", &vm]))?;
            break;
        }
        std::thread::sleep(h.poll);
    }
    let c = h.probe.confinement(project)?;
    Ok(LifecycleReport {
        project: project.to_string(),
        action: "stopped",
        confinement: c.as_str().to_string(),
    })
}

pub fn restart(h: &Host, project: &str) -> Result<LifecycleReport> {
    stop(h, project)?;
    let mut r = start(h, project)?;
    r.action = "restarted";
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, host, project};
    use crate::run::{Output, RecordingRunner};

    #[test]
    fn stop_boots_out_forwards_then_vm_then_stops() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        // Still Running after the agent's bootout (a hostagent that won't exit): limactl stop.
        for _ in 0..1000 {
            r.respond(
                "limactl",
                &["list"],
                Output::ok("{\"name\":\"bunker-t\",\"status\":\"Running\"}\n"),
            );
        }
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let _ = stop(&h, "t");
        let order: Vec<String> = r
            .calls()
            .into_iter()
            .filter(|c| {
                c.args
                    .first()
                    .is_some_and(|a| a == "bootout" || a == "stop")
            })
            .map(|c| format!("{} {}", c.program, c.args.join(" ")))
            .map(|s| s.split('/').next_back().unwrap().to_string())
            .collect();
        assert_eq!(
            order,
            [
                "com.bunker.t.tunnel",
                "com.bunker.t.fwd-proxy",
                "com.bunker.t.fwd-web",
                "com.bunker.t.vm",
                "limactl stop --tty=false bunker-t"
            ]
        );
    }

    #[test]
    fn stop_of_a_stopped_vm_skips_limactl_stop() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "limactl",
            &["list"],
            Output::ok("{\"name\":\"bunker-t\",\"status\":\"Stopped\"}\n"),
        );
        let h = host(
            &r,
            &crate::host::testing::FakeProbe(crate::lima::confine::Confinement::Stopped),
            d.path(),
        );
        project(&h, "t");
        assert_eq!(stop(&h, "t").unwrap().confinement, "stopped");
        assert!(
            !r.calls()
                .iter()
                .any(|c| c.program == "limactl" && c.args[0] == "stop")
        );
    }

    #[test]
    fn stop_waits_for_the_hostagent_instead_of_racing_limactl_stop() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let running = Output::ok("{\"name\":\"bunker-t\",\"status\":\"Running\"}\n");
        r.respond("limactl", &["list"], running.clone());
        r.respond("limactl", &["list"], running);
        r.respond(
            "limactl",
            &["list"],
            Output::ok("{\"name\":\"bunker-t\",\"status\":\"Stopped\"}\n"),
        );
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        stop(&h, "t").unwrap();
        assert!(
            !r.calls()
                .iter()
                .any(|c| c.program == "limactl" && c.args[0] == "stop"),
            "the bootout already stopped it"
        );
    }
}
