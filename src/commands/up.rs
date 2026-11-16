//! `bunker up <project>`: create and start the confined VM (phase A: VM, proxy, forwards).
use crate::config::Config;
use crate::error::Result;
use crate::host::Host;
use crate::launchd::{Agent, Role, ensure_in, kickstart, label};
use crate::lima::confine::Confinement;
use crate::lima::image::ensure_image;
use crate::lima::sandbox::{fwd_proxy_profile, fwd_web_profile, hostagent_profile};
use crate::lima::template::render_lima_yaml;
use crate::lima::vm::{
    agents_for, instance_status, lima_vm_dir, limactl_version, shell, vm_name, wait_running,
};
use crate::output::Report;
use crate::paths::{ProjectDir, ensure_private_dir, write_private};
use crate::run::Cmd;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::Duration;

#[derive(Debug)]
pub struct UpReport {
    pub project: String,
    pub vm: String,
    pub created: bool,
    pub confinement: String,
    pub warnings: Vec<String>,
}

impl Report for UpReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        let verb = if self.created {
            "created and started"
        } else {
            "up"
        };
        writeln!(
            w,
            "{}: {} {verb} ({})",
            self.project, self.vm, self.confinement
        )?;
        for x in &self.warnings {
            writeln!(w, "  warning: {x}")?;
        }
        Ok(())
    }
    fn json(&self) -> Value {
        json!({"project": self.project, "vm": self.vm, "created": self.created, "confinement": self.confinement, "warnings": self.warnings})
    }
    fn exit(&self) -> crate::output::ExitCode {
        if self.confinement == "unconfined" {
            crate::output::ExitCode::Unconfined
        } else {
            crate::output::ExitCode::Ok
        }
    }
}

/// The guest can't resolve its own hostname without DNS (plan 1), so sudo complains; pin it.
const HOSTS_SCRIPT: &str = r#"set -e
h=$(hostname)
if grep -q '^127\.0\.1\.1[[:space:]]' /etc/hosts; then
  sed -i "s/^127\.0\.1\.1[[:space:]].*/127.0.1.1 $h/" /etc/hosts
else
  echo "127.0.1.1 $h" >> /etc/hosts
fi
"#;

pub fn up(h: &Host, project: &str) -> Result<UpReport> {
    let pd = ProjectDir::under(&h.home, project);
    let cfg = Config::load(&pd)?;
    limactl_version(h)?;
    let image = ensure_image(h.r, &h.home.join("images"))?;
    let vm = vm_name(project);
    ensure_private_dir(&pd.logs_dir())?;
    let mut warnings = Vec::new();

    let yaml = render_lima_yaml(&cfg, &image, &vm);
    let size = |y: &str| -> Vec<String> {
        y.lines()
            .filter(|l| {
                l.starts_with("cpus:") || l.starts_with("memory:") || l.starts_with("disk:")
            })
            .map(str::to_string)
            .collect()
    };
    let previous = std::fs::read_to_string(pd.lima_yaml()).ok();
    write_private(&pd.lima_yaml(), yaml.as_bytes())?;
    let created = instance_status(h, &vm)?.is_none();
    if created {
        h.r.run_ok(&Cmd::new(h.limactl()).args([
            "create".to_string(),
            "--tty=false".into(),
            "--name".into(),
            vm.clone(),
            pd.lima_yaml().to_string_lossy().into_owned(),
        ]))?;
    } else if previous.is_some_and(|p| size(&p) != size(&yaml)) {
        warnings.push(format!(
            "[vm] size changed, but {vm} keeps the size it was created with; recreating it is a later feature"
        ));
    }

    let vm_dir = lima_vm_dir(&h.lima_home, &vm)?;
    // The hostagent can write the VM dir: never follow whatever it left at tmp.
    let ha_tmp = crate::lima::sandbox::hostagent_tmp(&vm_dir);
    if std::fs::symlink_metadata(&ha_tmp).is_ok_and(|m| !m.file_type().is_dir()) {
        std::fs::remove_file(&ha_tmp).map_err(|e| {
            crate::error::BunkerError::io(format!("cannot remove {}", ha_tmp.display()), e)
        })?;
    }
    ensure_private_dir(&ha_tmp)?;
    crate::lima::vm::seal_instance_yaml(&vm_dir)?;
    write_private(
        &pd.sandbox_sb(),
        hostagent_profile(&vm_dir, cfg.ports.ssh, &h.limactl).as_bytes(),
    )?;
    write_private(
        &pd.fwd_proxy_sb(),
        fwd_proxy_profile(cfg.ports.ssh, cfg.ports.proxy).as_bytes(),
    )?;
    write_private(
        &pd.fwd_web_sb(),
        fwd_web_profile(cfg.ports.ssh, cfg.ports.web).as_bytes(),
    )?;

    ensure_proxy(h, cfg.ports.proxy)?;
    let [vm_agent, fwd_proxy, fwd_web] = agents_for(project, &cfg, &pd, &h.limactl, &vm_dir);
    ensure_in(h.r, &h.agents_dir, &vm_agent)?;
    wait_running(h, &vm)?;
    ensure_in(h.r, &h.agents_dir, &fwd_proxy)?;
    ensure_in(h.r, &h.agents_dir, &fwd_web)?;
    h.r.run_ok(&shell(h, &vm).args(["sudo", "sh", "-s"]).stdin(HOSTS_SCRIPT))?;

    let confinement = h.probe.confinement(project)?;
    if let Confinement::Unconfined { reason, .. } = &confinement {
        warnings.push(reason.clone());
    }
    Ok(UpReport {
        project: project.to_string(),
        vm,
        created,
        confinement: confinement.as_str().to_string(),
        warnings,
    })
}

/// The shared proxy agent. It binds every project's port at start, so a project it isn't serving
/// yet means a restart.
pub fn proxy_agent(h: &Host) -> Agent {
    let logs = h.home.join("logs");
    Agent {
        label: label(None, Role::Proxy),
        program_args: vec![
            h.bunker_exe.to_string_lossy().into_owned(),
            "proxy".into(),
            "serve".into(),
        ],
        env: BTreeMap::from([(
            "BUNKER_HOME".to_string(),
            h.home.to_string_lossy().into_owned(),
        )]),
        keep_alive: true,
        run_at_load: true,
        throttle_secs: Some(5),
        stdout: logs.join("proxy.out.log"),
        stderr: logs.join("proxy.err.log"),
    }
}

fn ensure_proxy(h: &Host, port: u16) -> Result<()> {
    ensure_private_dir(&h.home.join("logs"))?;
    let installed = ensure_in(h.r, &h.agents_dir, &proxy_agent(h))?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listening = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok();
    if !installed && !listening {
        kickstart(h.r, &proxy_agent(h).label)?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, host, project};
    use crate::run::{Cmd, Output, RecordingRunner};

    pub const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    /// Queues the answers `up` needs: version, image, two `list` calls (exists?, running?).
    pub fn scripted(version: &str, listed: bool) -> RecordingRunner {
        let r = RecordingRunner::new();
        r.respond(
            "limactl",
            &["--version"],
            Output::ok(format!("limactl version {version}\n")),
        );
        r.respond(
            "/usr/bin/curl",
            &["-fsSL"],
            Output::ok(format!(
                "{EMPTY_SHA} *ubuntu-24.04-server-cloudimg-arm64.img\n"
            )),
        );
        r.respond_with("/usr/bin/curl", &["-fL"], |c: &Cmd| {
            let i = c.args.iter().position(|a| a == "-o").unwrap();
            std::fs::write(&c.args[i + 1], b"").unwrap();
            Output::ok("")
        });
        let running = "{\"name\":\"bunker-t\",\"status\":\"Running\",\"dir\":\"/x\"}\n";
        r.respond(
            "limactl",
            &["list"],
            Output::ok(if listed { running } else { "" }),
        );
        r.respond("limactl", &["list"], Output::ok(running));
        r.respond(
            "launchctl",
            &["print"],
            Output::fail(113, "Could not find service"),
        );
        r
    }

    #[test]
    fn up_refuses_old_lima() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.2.0", false);
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let e = up(&h, "t").unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("lima_too_old", crate::output::ExitCode::Failure)
        );
        assert_eq!(r.calls().len(), 1, "nothing else runs");
    }

    #[test]
    fn up_creates_then_starts_through_the_agent() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let h = host(&r, &CONFINED, d.path());
        let pd = project(&h, "t");
        let rep = up(&h, "t").unwrap();
        assert_eq!((rep.created, rep.confinement.as_str()), (true, "confined"));
        let calls = r.calls();
        let create = calls
            .iter()
            .position(|c| {
                c.program == "limactl" && c.args.first().map(String::as_str) == Some("create")
            })
            .unwrap();
        assert_eq!(
            calls[create].args,
            [
                "create",
                "--tty=false",
                "--name",
                "bunker-t",
                &pd.lima_yaml().to_string_lossy()
            ]
        );
        for f in [
            pd.lima_yaml(),
            pd.sandbox_sb(),
            pd.fwd_proxy_sb(),
            pd.fwd_web_sb(),
        ] {
            assert!(f.exists(), "{}", f.display());
        }
        for l in [
            "com.bunker.proxy",
            "com.bunker.t.vm",
            "com.bunker.t.fwd-proxy",
            "com.bunker.t.fwd-web",
        ] {
            assert!(h.agents_dir.join(format!("{l}.plist")).exists(), "{l}");
        }
        let hosts = calls
            .iter()
            .find(|c| {
                c.stdin
                    .as_deref()
                    .is_some_and(|s| String::from_utf8_lossy(s).contains("127.0.1.1"))
            })
            .expect("/etc/hosts write");
        assert_eq!(hosts.args[..3], ["shell", "--tty=false", "--workdir"]);
    }

    #[test]
    fn up_rerun_does_not_recreate() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", true);
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        assert!(!up(&h, "t").unwrap().created);
        assert!(!r.calls().iter().any(
            |c| c.program == "limactl" && c.args.first().map(String::as_str) == Some("create")
        ));
    }

    #[test]
    fn up_never_runs_unwrapped_start() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        up(&h, "t").unwrap();
        assert!(
            !r.calls()
                .iter()
                .any(|c| c.program == "limactl"
                    && c.args.first().map(String::as_str) == Some("start"))
        );
    }

    #[test]
    fn up_times_out_when_the_vm_never_runs() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "limactl",
            &["--version"],
            Output::ok("limactl version 2.3.0"),
        );
        r.respond(
            "/usr/bin/curl",
            &["-fsSL"],
            Output::ok(format!(
                "{EMPTY_SHA} *ubuntu-24.04-server-cloudimg-arm64.img\n"
            )),
        );
        r.respond_with("/usr/bin/curl", &["-fL"], |c: &Cmd| {
            let i = c.args.iter().position(|a| a == "-o").unwrap();
            std::fs::write(&c.args[i + 1], b"").unwrap();
            Output::ok("")
        });
        for _ in 0..1000 {
            r.respond(
                "limactl",
                &["list"],
                Output::ok("{\"name\":\"bunker-t\",\"status\":\"Stopped\"}\n"),
            );
        }
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        assert_eq!(up(&h, "t").unwrap_err().code, "vm_start_timeout");
    }
}

#[cfg(test)]
mod symlink_tests {
    use super::tests::scripted;
    use super::*;
    use crate::host::testing::{CONFINED, host, project};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn symlinked_hostagent_tmp_is_replaced_not_followed() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let victim = d.path().join("victim");
        std::fs::create_dir(&victim).unwrap();
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o755)).unwrap();
        let vm_dir = h.lima_home.join("bunker-t");
        std::fs::create_dir_all(&vm_dir).unwrap();
        std::os::unix::fs::symlink(&victim, vm_dir.join("tmp")).unwrap();
        up(&h, "t").unwrap();
        assert_eq!(
            std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(
            std::fs::symlink_metadata(vm_dir.join("tmp"))
                .unwrap()
                .is_dir()
        );
    }
}
