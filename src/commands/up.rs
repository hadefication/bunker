//! `bunker up <project>`: create and start the confined VM (phase A: VM, proxy, forwards), then
//! provision it (phase B: packages, users, nginx, php-fpm, MySQL, units).
use crate::config::Config;
use crate::error::{BunkerError, Result};
use crate::guest::provision::provision;
use crate::host::Host;
use crate::launchd::{Agent, Role, ensure_in, kickstart, label};
use crate::lima::confine::Confinement;
use crate::lima::image::ensure_image;
use crate::lima::sandbox::{fwd_proxy_profile, fwd_web_profile, hostagent_profile};
use crate::lima::template::render_lima_yaml;
use crate::lima::vm::{
    agents_for, instance_status, lima_vm_dir, limactl_version, shell, vm_name, wait_running,
};
use crate::output::ExitCode;
use crate::output::Report;
use crate::paths::{ProjectDir, ensure_private_dir, write_private};
use crate::run::Cmd;
use crate::secrets::store::SecretStore;
use crate::tunnel::cloudflared::Cloudflared;
use crate::tunnel::config::render_cloudflared_yml;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::Duration;

#[derive(Debug)]
pub struct UpReport {
    pub project: String,
    pub vm: String,
    pub created: bool,
    pub provisioned: bool,
    /// The domain the tunnel serves, when the project has one.
    pub tunnel: Option<String>,
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
            "{}: {} {verb} ({}{})",
            self.project,
            self.vm,
            self.confinement,
            if self.provisioned {
                ", provisioned"
            } else {
                ""
            }
        )?;
        if let Some(d) = &self.tunnel {
            writeln!(w, "  tunnel: https://{d}")?;
        }
        for x in &self.warnings {
            writeln!(w, "  warning: {x}")?;
        }
        Ok(())
    }
    fn json(&self) -> Value {
        json!({"project": self.project, "vm": self.vm, "created": self.created, "provisioned": self.provisioned, "tunnel": self.tunnel, "confinement": self.confinement, "warnings": self.warnings})
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

/// Phase A (VM, proxy, forwards), then phase B (provisioning) once the VM is confirmed confined.
pub fn up(
    h: &Host,
    store: &dyn SecretStore,
    project: &str,
    overwrite_dns: bool,
) -> Result<UpReport> {
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
    // Seal only what bunker just created. An existing instance must already be sealed: sealing an
    // unsealed one would bless whatever is in it now.
    if created {
        crate::lima::vm::seal_instance_yaml(&vm_dir)?;
    } else {
        crate::lima::vm::check_sealed(&vm_dir)?;
    }
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
    crate::lima::vm::wait_guest_ready(h, &vm)?;
    h.r.run_ok(&shell(h, &vm).args(["sudo", "sh", "-s"]).stdin(HOSTS_SCRIPT))?;

    if h.bunker_exe.components().any(|c| c.as_os_str() == "target") {
        warnings.push(format!(
            "the proxy and tunnel agents run {}, a build artifact under target/; install bunker so a rebuild or cargo clean can't break them",
            h.bunker_exe.display()
        ));
    }
    let confinement = h.probe.confinement(project)?;
    let mut tunnel = None;
    let provisioned = match &confinement {
        Confinement::Confined { .. } => {
            provision(h, store, &pd, &cfg)?;
            // Phase C: only a confined, provisioned VM is put on the internet.
            tunnel = ensure_tunnel(h, &pd, &cfg, overwrite_dns)?;
            true
        }
        Confinement::Unconfined { reason, .. } => {
            warnings.push(format!("{reason}; not provisioning an unconfined VM"));
            sync_tunnel(h, &pd, &cfg, &confinement)?;
            false
        }
        Confinement::Stopped => {
            warnings.push("the VM stopped before provisioning".into());
            sync_tunnel(h, &pd, &cfg, &confinement)?;
            false
        }
    };
    Ok(UpReport {
        project: project.to_string(),
        vm,
        created,
        provisioned,
        tunnel,
        confinement: confinement.as_str().to_string(),
        warnings,
    })
}

/// Phase C, for a project with both `domain` and `tunnel`: find or create the named tunnel, point
/// the domain at it, write cloudflared.yml and run cloudflared as a LaunchAgent.
fn ensure_tunnel(
    h: &Host,
    pd: &ProjectDir,
    cfg: &Config,
    overwrite_dns: bool,
) -> Result<Option<String>> {
    let (Some(domain), Some(name)) = (&cfg.domain, &cfg.tunnel) else {
        return Ok(None);
    };
    if !h.cloudflared_cert.exists() {
        return Err(BunkerError::new(
            "cloudflared_login_needed",
            ExitCode::Refused,
            format!(
                "{} is missing, so cloudflared can't manage tunnels",
                h.cloudflared_cert.display()
            ),
        )
        .hint("run cloudflared tunnel login (it opens a browser to pick the zone), then bunker up again"));
    }
    let cf = Cloudflared {
        r: h.r,
        bin: h.cloudflared.clone(),
        cert: h.cloudflared_cert.clone(),
    };
    let creds = pd.tunnel_credentials();
    let id = match cf.find(name)? {
        Some(id) if creds.exists() => id,
        Some(_) => {
            return Err(BunkerError::new(
                "tunnel_credentials_missing",
                ExitCode::Refused,
                format!(
                    "tunnel {name} exists but {} doesn't, so bunker can't run it",
                    creds.display()
                ),
            )
            .hint(format!(
                "copy its credentials JSON to {}, or delete it (cloudflared tunnel delete {name}) and run bunker up again",
                creds.display()
            )));
        }
        None => {
            cf.create(name, &creds)?;
            cf.find(name)?.ok_or_else(|| {
                BunkerError::new(
                    "cloudflared_failed",
                    ExitCode::Failure,
                    format!("created tunnel {name}, but cloudflared doesn't list it"),
                )
            })?
        }
    };
    cf.route(&id, domain, overwrite_dns)?;
    write_private(
        &pd.cloudflared_yml(),
        render_cloudflared_yml(&id, &creds, domain, cfg.ports.web).as_bytes(),
    )?;
    ensure_in(h.r, &h.agents_dir, &tunnel_agent(h, pd))?;
    Ok(Some(domain.clone()))
}

/// Runs the tunnel agent only while the VM is confined and the tunnel is set up (config,
/// cloudflared.yml and credentials); otherwise removes it, so the domain never points at a web
/// port nothing confined is serving (another local process could take it).
pub fn sync_tunnel(h: &Host, pd: &ProjectDir, cfg: &Config, c: &Confinement) -> Result<()> {
    let ready = cfg.domain.is_some()
        && cfg.tunnel.is_some()
        && pd.cloudflared_yml().exists()
        && pd.tunnel_credentials().exists();
    if ready && matches!(c, Confinement::Confined { .. }) {
        ensure_in(h.r, &h.agents_dir, &tunnel_agent(h, pd)).map(|_| ())
    } else {
        crate::launchd::remove_in(h.r, &h.agents_dir, &label(Some(&pd.name), Role::Tunnel))
    }
}

/// What the tunnel agent runs (`bunker tunnel-run <project>`): cloudflared, but only once the VM
/// is confined. At login launchd starts every agent at once with no check of its own, so the
/// domain would otherwise reach the Mac before anyone knows the VM came back confined. Refused,
/// the agent exits and launchd retries after its throttle.
pub fn tunnel_gate(h: &Host, project: &str) -> Result<Cmd> {
    let pd = ProjectDir::under(&h.home, project);
    match h.probe.confinement(project)? {
        Confinement::Confined { .. } => Ok(Cmd::new(h.cloudflared.to_string_lossy())
            .args(["tunnel", "--no-autoupdate", "--config"])
            .arg(pd.cloudflared_yml().to_string_lossy())
            .arg("run")),
        other => Err(BunkerError::new(
            "tunnel_held",
            ExitCode::Refused,
            format!(
                "not starting {project}'s tunnel: its VM is {}",
                other.as_str()
            ),
        )),
    }
}

/// How often `tunnel-run` re-checks confinement while cloudflared runs.
pub const TUNNEL_CHECK: std::time::Duration = std::time::Duration::from_secs(5);

/// What `watch_tunnel` needs from cloudflared's process.
pub trait Supervised {
    /// The exit code once it has exited.
    fn try_wait(&mut self) -> std::io::Result<Option<i32>>;
    fn kill(&mut self) -> std::io::Result<()>;
}

impl Supervised for std::process::Child {
    fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        Ok(std::process::Child::try_wait(self)?.map(|s| s.code().unwrap_or(1)))
    }
    fn kill(&mut self) -> std::io::Result<()> {
        std::process::Child::kill(self)?;
        std::process::Child::wait(self).map(|_| ())
    }
}

/// Keeps the tunnel up only while the VM stays confined: re-checks every `every`, kills
/// cloudflared and fails the moment it isn't (launchd restarts the agent, whose gate then holds
/// it). Returns cloudflared's own exit code if it exits first.
pub fn watch_tunnel(
    h: &Host,
    project: &str,
    child: &mut dyn Supervised,
    every: std::time::Duration,
) -> Result<i32> {
    loop {
        if let Some(code) = child
            .try_wait()
            .map_err(|e| BunkerError::io("cannot check on cloudflared", e))?
        {
            return Ok(code);
        }
        std::thread::sleep(every);
        let state = match h.probe.confinement(project) {
            Ok(Confinement::Confined { .. }) => continue,
            Ok(other) => other.as_str().to_string(),
            Err(e) => format!("unknown ({})", e.message),
        };
        let _ = child.kill();
        return Err(BunkerError::new(
            "tunnel_held",
            ExitCode::Refused,
            format!("stopped {project}'s tunnel: its VM is {state}"),
        ));
    }
}

/// `com.bunker.<project>.tunnel`: cloudflared with bunker's config, restarted if it exits.
pub fn tunnel_agent(h: &Host, pd: &ProjectDir) -> Agent {
    let logs = pd.logs_dir();
    Agent {
        label: label(Some(&pd.name), Role::Tunnel),
        program_args: vec![
            h.bunker_exe.to_string_lossy().into_owned(),
            "tunnel-run".into(),
            pd.name.clone(),
        ],
        env: BTreeMap::from([(
            "BUNKER_HOME".to_string(),
            h.home.to_string_lossy().into_owned(),
        )]),
        keep_alive: true,
        run_at_load: true,
        throttle_secs: Some(5),
        stdout: logs.join("tunnel.out.log"),
        stderr: logs.join("tunnel.err.log"),
    }
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
    use crate::host::testing::{CONFINED, host};

    /// The fixture project without `domain` and `tunnel`, so phase C (cloudflared) stays out.
    pub fn project(h: &Host, name: &str) -> ProjectDir {
        let pd = crate::host::testing::project(h, name);
        let text = std::fs::read_to_string(pd.config()).unwrap();
        let text: String = text
            .lines()
            .filter(|l| !l.starts_with("domain") && !l.starts_with("tunnel"))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(pd.config(), text).unwrap();
        pd
    }
    use crate::run::{Cmd, Output, RecordingRunner};
    use crate::secrets::store::MemoryStore;

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
        let e = up(&h, &MemoryStore::default(), "t", false).unwrap_err();
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
        let rep = up(&h, &MemoryStore::default(), "t", false).unwrap();
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
        assert!(!up(&h, &MemoryStore::default(), "t", false).unwrap().created);
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
        up(&h, &MemoryStore::default(), "t", false).unwrap();
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
        assert_eq!(
            up(&h, &MemoryStore::default(), "t", false)
                .unwrap_err()
                .code,
            "vm_start_timeout"
        );
    }
}

#[cfg(test)]
mod symlink_tests {
    use super::tests::{project, scripted};
    use super::*;
    use crate::host::testing::{CONFINED, host};
    use crate::run::Output;
    use crate::run::RecordingRunner;
    use crate::secrets::store::MemoryStore;
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
        up(&h, &MemoryStore::default(), "t", false).unwrap();
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

    #[test]
    fn existing_unsealed_instance_is_refused_not_sealed() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", true);
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let vm_dir = h.lima_home.join("bunker-t");
        std::fs::create_dir_all(&vm_dir).unwrap();
        std::fs::write(vm_dir.join("lima.yaml"), "mounts: [{location: \"~\"}]\n").unwrap();
        assert_eq!(
            up(&h, &MemoryStore::default(), "t", false)
                .unwrap_err()
                .code,
            "instance_tampered"
        );
        assert!(
            crate::lima::vm::check_sealed(&vm_dir).is_err(),
            "never blessed"
        );
    }

    fn provisioned(r: &RecordingRunner) -> bool {
        r.calls().iter().any(|c| {
            c.program == "limactl"
                && String::from_utf8_lossy(c.stdin.as_deref().unwrap_or_default())
                    .contains("bunker_provision")
        })
    }

    #[test]
    fn up_provisions_a_confined_vm() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let rep = up(&h, &MemoryStore::default(), "t", false).unwrap();
        assert!(rep.provisioned && provisioned(&r));
        // The probe ran before the provision call, not after.
        let calls = r.calls();
        let hosts = calls
            .iter()
            .position(|c| c.args.ends_with(&["sudo".into(), "sh".into(), "-s".into()]))
            .unwrap();
        let prov = calls
            .iter()
            .position(|c| {
                String::from_utf8_lossy(c.stdin.as_deref().unwrap_or_default())
                    .contains("bunker_provision")
            })
            .unwrap();
        assert!(hosts < prov);
    }

    #[test]
    fn up_never_provisions_an_unconfined_vm() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let probe = crate::host::testing::FakeProbe(Confinement::Unconfined {
            hostagent: 1,
            reason: "not sandboxed".into(),
        });
        let h = host(&r, &probe, d.path());
        project(&h, "t");
        let rep = up(&h, &MemoryStore::default(), "t", false).unwrap();
        assert_eq!(rep.exit(), crate::output::ExitCode::Unconfined);
        assert!(!rep.provisioned && !provisioned(&r));
    }

    #[test]
    fn up_without_domain_skips_tunnel() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        up(&h, &MemoryStore::default(), "t", false).unwrap();
        assert!(!r.calls().iter().any(|c| c.program == "cloudflared"));
        assert!(!h.agents_dir.join("com.bunker.t.tunnel.plist").exists());
    }

    /// Answers `cloudflared tunnel ... list` with `lists` in turn, creates the credentials file.
    fn cloudflared(r: &RecordingRunner, lists: &[&'static str]) {
        for &l in lists {
            r.respond_with("cloudflared", &["tunnel"], move |c| {
                if c.args.iter().any(|a| a == "create") {
                    let i = c
                        .args
                        .iter()
                        .position(|a| a == "--credentials-file")
                        .unwrap();
                    std::fs::write(&c.args[i + 1], "{}").unwrap();
                }
                Output::ok(l)
            });
        }
    }

    #[test]
    fn up_with_domain_creates_routes_and_runs_the_tunnel() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let h = host(&r, &CONFINED, d.path());
        let pd = crate::host::testing::project(&h, "t");
        std::fs::write(&h.cloudflared_cert, "x").unwrap();
        // list (none), create, list (found), route
        cloudflared(
            &r,
            &["[]", "", "[{\"id\":\"6ff4\",\"name\":\"life-os\"}]", ""],
        );
        let rep = up(&h, &MemoryStore::default(), "t", false).unwrap();
        assert!(
            rep.tunnel.as_deref() == Some("life-os.bangkila.com"),
            "{rep:?}"
        );
        let subs: Vec<String> = r
            .calls()
            .iter()
            .filter(|c| c.program == "cloudflared")
            .map(|c| c.args[3].clone())
            .collect();
        assert_eq!(subs, ["list", "create", "list", "route"]);
        let yml = std::fs::read_to_string(pd.cloudflared_yml()).unwrap();
        assert!(yml.contains("tunnel: 6ff4") && yml.contains("hostname: life-os.bangkila.com"));
        let plist =
            std::fs::read_to_string(h.agents_dir.join("com.bunker.t.tunnel.plist")).unwrap();
        for a in ["<string>tunnel-run</string>", "<string>t</string>"] {
            assert!(plist.contains(a), "{a} missing from\n{plist}");
        }
    }

    #[test]
    fn up_with_domain_needs_cloudflared_login() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let h = host(&r, &CONFINED, d.path());
        crate::host::testing::project(&h, "t");
        let e = up(&h, &MemoryStore::default(), "t", false).unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("cloudflared_login_needed", crate::output::ExitCode::Refused)
        );
        assert!(e.hint.unwrap().contains("cloudflared tunnel login"));
    }

    #[test]
    fn existing_tunnel_without_credentials_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let h = host(&r, &CONFINED, d.path());
        crate::host::testing::project(&h, "t");
        std::fs::write(&h.cloudflared_cert, "x").unwrap();
        cloudflared(&r, &["[{\"id\":\"6ff4\",\"name\":\"life-os\"}]"]);
        let e = up(&h, &MemoryStore::default(), "t", false).unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            (
                "tunnel_credentials_missing",
                crate::output::ExitCode::Refused
            )
        );
        assert!(
            !r.calls()
                .iter()
                .any(|c| c.program == "cloudflared" && c.args.iter().any(|a| a == "create"))
        );
    }

    fn tunnel_ready(h: &Host) -> ProjectDir {
        let pd = crate::host::testing::project(h, "t");
        std::fs::write(pd.cloudflared_yml(), "x").unwrap();
        std::fs::write(pd.tunnel_credentials(), "{}").unwrap();
        pd
    }

    #[test]
    fn sync_tunnel_runs_it_only_for_a_confined_vm() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        let pd = tunnel_ready(&h);
        let cfg = Config::load(&pd).unwrap();
        let plist = h.agents_dir.join("com.bunker.t.tunnel.plist");
        sync_tunnel(&h, &pd, &cfg, &CONFINED.0).unwrap();
        assert!(plist.exists());
        let unconfined = Confinement::Unconfined {
            hostagent: 1,
            reason: "x".into(),
        };
        sync_tunnel(&h, &pd, &cfg, &unconfined).unwrap();
        assert!(
            !plist.exists(),
            "an unconfined VM is taken off the internet"
        );
        sync_tunnel(&h, &pd, &cfg, &CONFINED.0).unwrap();
        sync_tunnel(&h, &pd, &cfg, &Confinement::Stopped).unwrap();
        assert!(!plist.exists());
    }

    #[test]
    fn up_of_an_unconfined_vm_removes_its_tunnel() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let probe = crate::host::testing::FakeProbe(Confinement::Unconfined {
            hostagent: 1,
            reason: "x".into(),
        });
        let h = host(&r, &probe, d.path());
        tunnel_ready(&h);
        std::fs::create_dir_all(&h.agents_dir).unwrap();
        std::fs::write(h.agents_dir.join("com.bunker.t.tunnel.plist"), "x").unwrap();
        let rep = up(&h, &MemoryStore::default(), "t", false).unwrap();
        assert!(rep.tunnel.is_none());
        assert!(!h.agents_dir.join("com.bunker.t.tunnel.plist").exists());
    }

    #[test]
    fn every_agent_runs_at_load() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        let pd = crate::host::testing::project(&h, "t");
        let cfg = Config::load(&pd).unwrap();
        let mut agents: Vec<Agent> = crate::lima::vm::agents_for(
            "t",
            &cfg,
            &pd,
            &h.limactl,
            std::path::Path::new("/x/bunker-t"),
        )
        .to_vec();
        agents.push(proxy_agent(&h));
        agents.push(tunnel_agent(&h, &pd));
        for a in agents {
            assert!(
                a.run_at_load && a.keep_alive && a.throttle_secs.is_some(),
                "{}",
                a.label
            );
            assert!(
                a.stdout.starts_with(h.home.join("logs")) || a.stdout.starts_with(pd.logs_dir()),
                "{}",
                a.label
            );
        }
    }

    #[test]
    fn proxy_agent_points_at_installed_binary() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("Cellar/bunker/2.0.0/bin/bunker");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "").unwrap();
        let link = d.path().join("bin-bunker");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(
            crate::host::installed_exe(&link),
            real.canonicalize().unwrap()
        );
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        assert_eq!(
            proxy_agent(&h).program_args[..3],
            ["/usr/local/bin/bunker", "proxy", "serve"]
        );
    }

    #[test]
    fn up_warns_when_binary_is_a_build_artifact() {
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let mut h = host(&r, &CONFINED, d.path());
        h.bunker_exe = "/x/repo/target/release/bunker".into();
        project(&h, "t");
        let rep = up(&h, &MemoryStore::default(), "t", false).unwrap();
        assert!(
            rep.warnings
                .iter()
                .any(|w| w.contains("target/") && w.contains("/x/repo/target/release/bunker")),
            "{:?}",
            rep.warnings
        );
        let d2 = tempfile::tempdir().unwrap();
        let r2 = scripted("2.3.0", false);
        let h2 = host(&r2, &CONFINED, d2.path());
        project(&h2, "t");
        let rep = up(&h2, &MemoryStore::default(), "t", false).unwrap();
        assert!(!rep.warnings.iter().any(|w| w.contains("target/")));
    }

    #[test]
    fn tunnel_agent_runs_through_the_confinement_gate() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        let pd = crate::host::testing::project(&h, "t");
        let a = tunnel_agent(&h, &pd);
        assert_eq!(a.program_args, ["/usr/local/bin/bunker", "tunnel-run", "t"]);
        assert_eq!(
            a.env.get("BUNKER_HOME"),
            Some(&h.home.to_string_lossy().into_owned())
        );
    }

    /// Answers `confinement` from a script, then repeats its last answer.
    struct ScriptedProbe(std::sync::Mutex<Vec<Confinement>>);
    impl crate::lima::confine::ConfinementProbe for ScriptedProbe {
        fn confinement(&self, _p: &str) -> Result<Confinement> {
            let mut v = self.0.lock().unwrap();
            Ok(if v.len() > 1 {
                v.remove(0)
            } else {
                v[0].clone()
            })
        }
    }

    #[derive(Default)]
    struct FakeChild {
        exit_after: Option<usize>,
        polls: usize,
        killed: bool,
    }
    impl Supervised for FakeChild {
        fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
            self.polls += 1;
            Ok(match self.exit_after {
                Some(n) if self.polls > n => Some(3),
                _ if self.killed => Some(143),
                _ => None,
            })
        }
        fn kill(&mut self) -> std::io::Result<()> {
            self.killed = true;
            Ok(())
        }
    }

    #[test]
    fn tunnel_stops_once_the_vm_is_no_longer_confined() {
        // A manual limactl stop/start leaves cloudflared serving an unconfined VM unless the
        // gate keeps checking.
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let probe = ScriptedProbe(std::sync::Mutex::new(vec![
            Confinement::Confined { hostagent: 1 },
            Confinement::Confined { hostagent: 1 },
            Confinement::Unconfined {
                hostagent: 2,
                reason: "not sandboxed".into(),
            },
        ]));
        let h = host(&r, &probe, d.path());
        let mut child = FakeChild::default();
        let e = watch_tunnel(&h, "t", &mut child, std::time::Duration::from_millis(1)).unwrap_err();
        assert!(child.killed);
        assert_eq!((e.code, e.exit), ("tunnel_held", ExitCode::Refused));
        assert!(e.message.contains("unconfined"), "{}", e.message);
    }

    #[test]
    fn tunnel_watch_passes_cloudflareds_own_exit_through() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        let mut child = FakeChild {
            exit_after: Some(2),
            ..Default::default()
        };
        assert_eq!(
            watch_tunnel(&h, "t", &mut child, std::time::Duration::from_millis(1)).unwrap(),
            3
        );
        assert!(!child.killed);
    }

    #[test]
    fn tunnel_gate_starts_cloudflared_only_when_confined() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        let pd = crate::host::testing::project(&h, "t");
        let c = tunnel_gate(&h, "t").unwrap();
        assert_eq!(c.program, "cloudflared");
        assert_eq!(
            c.args,
            [
                "tunnel",
                "--no-autoupdate",
                "--config",
                &pd.cloudflared_yml().to_string_lossy(),
                "run"
            ]
        );
        let probe = crate::host::testing::FakeProbe(Confinement::Unconfined {
            hostagent: 1,
            reason: "x".into(),
        });
        let h = host(&r, &probe, d.path());
        let e = tunnel_gate(&h, "t").unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("tunnel_held", crate::output::ExitCode::Refused)
        );
        let h = host(
            &r,
            &crate::host::testing::FakeProbe(Confinement::Stopped),
            d.path(),
        );
        assert_eq!(tunnel_gate(&h, "t").unwrap_err().code, "tunnel_held");
    }

    #[test]
    fn up_waits_for_limas_boot_scripts_before_provisioning() {
        // Lima's boot scripts run in cloud-init's final stage and cut existing SSH sessions; a
        // session cut while waiting (exit 255) is retried.
        let d = tempfile::tempdir().unwrap();
        let r = scripted("2.3.0", false);
        let is_wait = |c: &crate::run::Cmd| {
            String::from_utf8_lossy(c.stdin.as_deref().unwrap_or_default())
                .contains("cloud-init status --wait")
        };
        let cut = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        for _ in 0..20 {
            let cut = cut.clone();
            r.respond_with("limactl", &["shell"], move |c| {
                if is_wait(c) && cut.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    Output::fail(255, "Connection to 127.0.0.1 closed by remote host.")
                } else {
                    Output::ok("")
                }
            });
        }
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        up(&h, &MemoryStore::default(), "t", false).unwrap();
        let calls = r.calls();
        let waits: Vec<usize> = calls
            .iter()
            .enumerate()
            .filter(|(_, c)| is_wait(c))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            waits.len(),
            2,
            "retried once after the cut session: {:#?}",
            calls.iter().map(|c| c.args.join(" ")).collect::<Vec<_>>()
        );
        let prov = calls
            .iter()
            .position(|c| {
                c.args
                    .ends_with(&["sudo".into(), "bash".into(), "-s".into()])
                    && !is_wait(c)
            })
            .unwrap();
        assert!(
            waits[1] < prov,
            "provisioning starts only after the guest is ready"
        );
        let script =
            String::from_utf8_lossy(calls[waits[0]].stdin.as_deref().unwrap()).into_owned();
        assert!(
            script.contains("/dev/tcp/127.0.0.1/3128"),
            "waits for the proxy forward too"
        );
    }
}
