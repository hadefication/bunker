//! limactl: version, instances, readiness, and the three launchd agents that run a VM confined.
use crate::config::Config;
use crate::error::{BunkerError, Result};
use crate::host::Host;
use crate::launchd::{Agent, Role, label};
use crate::output::ExitCode;
use crate::paths::ProjectDir;
use crate::run::Cmd;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const MIN_LIMA: (u32, u32, u32) = (2, 3, 0);

pub fn vm_name(project: &str) -> String {
    format!("bunker-{project}")
}

/// `limactl --version`; below 2.3.0 is `lima_too_old` (plan 1 proved containment on 2.3.0).
pub fn limactl_version(h: &Host) -> Result<(u32, u32, u32)> {
    let out =
        h.r.run_ok(&Cmd::new(h.limactl()).arg("--version"))?
            .stdout_str();
    let v = out
        .split_whitespace()
        .last()
        .and_then(|v| {
            let mut it = v
                .trim_start_matches('v')
                .split('.')
                .map(|p| p.parse::<u32>().ok());
            Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
        })
        .ok_or_else(|| {
            BunkerError::new(
                "lima_version",
                ExitCode::Failure,
                format!("cannot read limactl --version: {out:?}"),
            )
        })?;
    if v < MIN_LIMA {
        return Err(BunkerError::new(
            "lima_too_old",
            ExitCode::Failure,
            format!(
                "Lima {}.{}.{} is older than 2.3.0, the version bunker's containment was tested on",
                v.0, v.1, v.2
            ),
        )
        .hint("brew upgrade lima"));
    }
    Ok(v)
}

/// Realpath of `$LIMA_HOME/<name>` (of `$LIMA_HOME` joined with the name before it exists).
pub fn lima_vm_dir(lima_home: &Path, name: &str) -> Result<PathBuf> {
    let dir = lima_home.join(name);
    if let Ok(real) = dir.canonicalize() {
        return Ok(real);
    }
    std::fs::create_dir_all(lima_home)
        .map_err(|e| BunkerError::io(format!("cannot create {}", lima_home.display()), e))?;
    let base = lima_home
        .canonicalize()
        .map_err(|e| BunkerError::io(format!("cannot resolve {}", lima_home.display()), e))?;
    Ok(base.join(name))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub name: String,
    pub status: String,
}

/// `limactl list --format json`: one JSON object per line.
pub fn list_instances(h: &Host) -> Result<Vec<Instance>> {
    let out =
        h.r.run_ok(&Cmd::new(h.limactl()).args(["list", "--format", "json"]))?
            .stdout_str();
    Ok(out
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| {
            Some(Instance {
                name: v.get("name")?.as_str()?.to_string(),
                status: v
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect())
}

pub fn instance_status(h: &Host, vm: &str) -> Result<Option<String>> {
    Ok(list_instances(h)?
        .into_iter()
        .find(|i| i.name == vm)
        .map(|i| i.status))
}

/// `limactl shell` without a TTY, from `/` (there are no mounts to cd into).
pub fn shell(h: &Host, vm: &str) -> Cmd {
    Cmd::new(h.limactl()).args(["shell", "--tty=false", "--workdir", "/", vm])
}

/// The vm, fwd-proxy and fwd-web agents. launchd supervises all three, so the forwards reconnect
/// after a guest reboot, and the hostagent only ever starts under its Seatbelt profile.
pub fn agents_for(
    project: &str,
    cfg: &Config,
    pd: &ProjectDir,
    limactl: &Path,
    vm_dir: &Path,
) -> [Agent; 3] {
    let vm = vm_name(project);
    let logs = pd.logs_dir();
    let mut env = BTreeMap::new();
    if let Some(dir) = limactl.parent().filter(|d| !d.as_os_str().is_empty()) {
        env.insert(
            "PATH".to_string(),
            format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", dir.display()),
        );
    }
    if let Some(lh) = vm_dir.parent() {
        env.insert("LIMA_HOME".to_string(), lh.to_string_lossy().into_owned());
    }
    let agent = |role: Role, name: &str, args: Vec<String>| Agent {
        label: label(Some(project), role),
        program_args: args,
        env: env.clone(),
        keep_alive: true,
        run_at_load: true,
        throttle_secs: Some(5),
        stdout: logs.join(format!("{name}.out.log")),
        stderr: logs.join(format!("{name}.err.log")),
    };
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let ssh = |profile: PathBuf, flag: &str, spec: String| -> Vec<String> {
        let mut v: Vec<String> = vec![
            "/usr/bin/sandbox-exec".into(),
            "-f".into(),
            s(&profile),
            "/usr/bin/ssh".into(),
            "-F".into(),
            s(&vm_dir.join("ssh.config")),
        ];
        for o in [
            "ControlMaster=no",
            "ControlPath=none",
            "ForwardAgent=no",
            "ForwardX11=no",
            "PermitLocalCommand=no",
            "ExitOnForwardFailure=yes",
            "ServerAliveInterval=5",
            "ServerAliveCountMax=3",
        ] {
            v.push("-o".into());
            v.push(o.into());
        }
        v.extend(["-N".into(), flag.into(), spec, format!("lima-{vm}")]);
        v
    };
    let vm_args: Vec<String> = vec![
        "/usr/bin/sandbox-exec".into(),
        "-f".into(),
        s(&pd.sandbox_sb()),
        s(limactl),
        "start".into(),
        "--foreground".into(),
        "--tty=false".into(),
        vm.clone(),
    ];
    [
        agent(Role::Vm, "vm", vm_args),
        agent(
            Role::FwdProxy,
            "fwd-proxy",
            ssh(
                pd.fwd_proxy_sb(),
                "-R",
                format!("127.0.0.1:3128:127.0.0.1:{}", cfg.ports.proxy),
            ),
        ),
        agent(
            Role::FwdWeb,
            "fwd-web",
            ssh(
                pd.fwd_web_sb(),
                "-L",
                format!("127.0.0.1:{}:127.0.0.1:80", cfg.ports.web),
            ),
        ),
    ]
}

/// Waits (bounded by `h.wait_timeout`) for the instance to be Running and answer `limactl shell`.
pub fn wait_running(h: &Host, vm: &str) -> Result<()> {
    let deadline = Instant::now() + h.wait_timeout;
    loop {
        if instance_status(h, vm)?.as_deref() == Some("Running")
            && h.r.run(&shell(h, vm).arg("true"))?.success()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(BunkerError::new(
                "vm_start_timeout",
                ExitCode::Failure,
                format!("{vm} did not come up within {}s", h.wait_timeout.as_secs()),
            )
            .hint(format!(
                "see ~/.bunker/<project>/logs/vm.err.log, or run limactl list {vm}"
            )));
        }
        std::thread::sleep(h.poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn agents_have_expected_program_arguments() {
        let cfg = Config::parse(&crate::config::fixture_config()).unwrap();
        let pd = ProjectDir::under(Path::new("/h"), "t");
        let [vm, fp, fw] = agents_for(
            "t",
            &cfg,
            &pd,
            Path::new("/opt/homebrew/bin/limactl"),
            Path::new("/l/bunker-t"),
        );
        assert_eq!(vm.label, "com.bunker.t.vm");
        assert_eq!(
            vm.program_args,
            strings(&[
                "/usr/bin/sandbox-exec",
                "-f",
                "/h/t/sandbox.sb",
                "/opt/homebrew/bin/limactl",
                "start",
                "--foreground",
                "--tty=false",
                "bunker-t"
            ])
        );
        let ssh = |profile: &str, flag: &str, spec: &str| {
            strings(&[
                "/usr/bin/sandbox-exec",
                "-f",
                profile,
                "/usr/bin/ssh",
                "-F",
                "/l/bunker-t/ssh.config",
                "-o",
                "ControlMaster=no",
                "-o",
                "ControlPath=none",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ForwardX11=no",
                "-o",
                "PermitLocalCommand=no",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ServerAliveInterval=5",
                "-o",
                "ServerAliveCountMax=3",
                "-N",
                flag,
                spec,
                "lima-bunker-t",
            ])
        };
        assert_eq!(fp.label, "com.bunker.t.fwd-proxy");
        assert_eq!(
            fp.program_args,
            ssh("/h/t/fwd-proxy.sb", "-R", "127.0.0.1:3128:127.0.0.1:18800")
        );
        assert_eq!(fw.label, "com.bunker.t.fwd-web");
        assert_eq!(
            fw.program_args,
            ssh("/h/t/fwd-web.sb", "-L", "127.0.0.1:18900:127.0.0.1:80")
        );
        for a in [&vm, &fp, &fw] {
            assert!(
                a.keep_alive && a.run_at_load && a.throttle_secs == Some(5),
                "{}",
                a.label
            );
            assert!(a.stdout.starts_with("/h/t/logs") && a.stderr.starts_with("/h/t/logs"));
        }
    }

    #[test]
    fn fwd_ssh_never_multiplexes() {
        let cfg = Config::parse(&crate::config::fixture_config()).unwrap();
        let pd = ProjectDir::under(Path::new("/h"), "t");
        let [_, fp, fw] = agents_for(
            "t",
            &cfg,
            &pd,
            Path::new("limactl"),
            Path::new("/l/bunker-t"),
        );
        for a in [fp, fw] {
            assert!(a.program_args.contains(&"ControlMaster=no".to_string()));
            assert!(a.program_args.contains(&"ControlPath=none".to_string()));
        }
    }

    #[test]
    fn lima_vm_dir_resolves_before_and_after_create() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().canonicalize().unwrap();
        assert_eq!(
            lima_vm_dir(d.path(), "bunker-t").unwrap(),
            real.join("bunker-t")
        );
        std::fs::create_dir(d.path().join("bunker-t")).unwrap();
        assert_eq!(
            lima_vm_dir(d.path(), "bunker-t").unwrap(),
            real.join("bunker-t")
        );
    }
}
