//! `bunker destroy <project> [--yes]`: delete the VM, its agents and generated files. Keeps
//! `bunker.toml`, `backups/` and the logs.
use crate::error::{BunkerError, Result};
use crate::host::Host;
use crate::lima::vm::{instance_status, vm_name};
use crate::output::{ExitCode, Report};
use crate::paths::ProjectDir;
use crate::run::Cmd;
use serde_json::{Value, json};
use std::io::{self, Write};

pub struct DestroyReport {
    pub project: String,
    pub vm: String,
    pub notes: Vec<String>,
}

impl Report for DestroyReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "{}: deleted {}, its agents and generated files (bunker.toml kept)",
            self.project, self.vm
        )?;
        for n in &self.notes {
            writeln!(w, "  {n}")?;
        }
        Ok(())
    }
    fn json(&self) -> Value {
        json!({"project": self.project, "vm": self.vm, "notes": self.notes})
    }
}

/// Refuses unless `yes`, or the project name is typed back on a TTY.
pub fn confirm(project: &str, yes: bool) -> Result<()> {
    crate::config::validate::validate_project_name(project)?;
    if !ProjectDir::new(project).config().exists() {
        return Err(BunkerError::new(
            "project_not_found",
            ExitCode::NotFound,
            format!("no bunker project named {project:?}"),
        ));
    }
    if yes {
        return Ok(());
    }
    // SAFETY: isatty only inspects the descriptor.
    if unsafe { libc::isatty(0) } != 1 {
        return Err(BunkerError::new(
            "needs_tty",
            ExitCode::Refused,
            format!("destroying {project} needs confirmation"),
        )
        .hint("pass --yes, or run it in a terminal and type the project name"));
    }
    eprint!(
        "This deletes the VM bunker-{project}, its database and storage. Type {project} to confirm: "
    );
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| BunkerError::io("cannot read the confirmation", e))?;
    if line.trim() == project {
        Ok(())
    } else {
        Err(BunkerError::new(
            "not_confirmed",
            ExitCode::Refused,
            "the name did not match; nothing was deleted",
        ))
    }
}

pub fn destroy(h: &Host, project: &str) -> Result<DestroyReport> {
    let pd = ProjectDir::under(&h.home, project);
    crate::commands::lifecycle::stop(h, project)?;
    let vm = vm_name(project);
    if instance_status(h, &vm)?.is_some() {
        let vm_dir = crate::lima::vm::lima_vm_dir(&h.lima_home, &vm)?;
        crate::lima::vm::unseal_instance_yaml(&vm_dir)?;
        // On failure lima.yaml stays unsealed on purpose: re-sealing could bless a changed file,
        // and an unsealed instance is refused by the next up or start.
        h.r.run_ok(&Cmd::new(h.limactl()).args(["delete", "--force", "--tty=false", &vm]))?;
    }
    for f in [
        pd.lima_yaml(),
        pd.sandbox_sb(),
        pd.fwd_proxy_sb(),
        pd.fwd_web_sb(),
        pd.cloudflared_yml(),
    ] {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_dir_all(pd.leases_dir());
    Ok(DestroyReport {
        project: project.to_string(),
        vm,
        notes: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, host, project};
    use crate::run::{Output, RecordingRunner};

    #[test]
    fn destroy_deletes_vm_and_generated_files_but_keeps_config() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "limactl",
            &["list"],
            Output::ok("{\"name\":\"bunker-t\",\"status\":\"Stopped\"}\n"),
        );
        r.respond(
            "limactl",
            &["list"],
            Output::ok("{\"name\":\"bunker-t\",\"status\":\"Stopped\"}\n"),
        );
        let h = host(&r, &CONFINED, d.path());
        let pd = project(&h, "t");
        std::fs::write(pd.lima_yaml(), "x").unwrap();
        std::fs::write(pd.sandbox_sb(), "x").unwrap();
        destroy(&h, "t").unwrap();
        assert!(pd.config().exists() && !pd.lima_yaml().exists() && !pd.sandbox_sb().exists());
        assert!(r.calls().iter().any(|c| c.program == "limactl"
            && c.args == ["delete", "--force", "--tty=false", "bunker-t"]));
    }

    #[test]
    fn failed_delete_leaves_lima_yaml_unsealed_for_up_to_refuse() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let stopped = Output::ok("{\"name\":\"bunker-t\",\"status\":\"Stopped\"}\n");
        r.respond("limactl", &["list"], stopped.clone());
        r.respond("limactl", &["list"], stopped);
        r.respond("limactl", &["delete"], Output::fail(1, "busy"));
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let vm_dir = h.lima_home.join("bunker-t");
        std::fs::create_dir_all(&vm_dir).unwrap();
        std::fs::write(vm_dir.join("lima.yaml"), "x").unwrap();
        crate::lima::vm::seal_instance_yaml(&vm_dir).unwrap();
        assert!(destroy(&h, "t").is_err());
        assert_eq!(
            crate::lima::vm::check_sealed(&vm_dir).unwrap_err().code,
            "instance_tampered"
        );
    }
}
