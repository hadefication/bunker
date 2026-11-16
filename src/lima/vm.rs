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
    let mut tmp = crate::lima::sandbox::hostagent_tmp(vm_dir)
        .to_string_lossy()
        .into_owned();
    tmp.push('/');
    env.insert("TMPDIR".to_string(), tmp);
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

/// Lima rereads the instance's lima.yaml on every start, including the restarts launchd does on
/// its own. The hostagent may write the rest of the VM dir, so the file is made user-immutable:
/// macOS then refuses writes, renames and hard links to it (a path-only deny could be bypassed by
/// linking it from the writable dir). A file with extra links is never sealed.
pub fn seal_instance_yaml(vm_dir: &Path) -> Result<()> {
    set_immutable(vm_dir, true)
}

/// Clears the flag so `limactl delete` (and future `limactl edit`) can change the instance.
pub fn unseal_instance_yaml(vm_dir: &Path) -> Result<()> {
    set_immutable(vm_dir, false)
}

fn tampered(msg: String) -> BunkerError {
    BunkerError::new("instance_tampered", ExitCode::Refused, msg)
        .hint("inspect it, then bunker destroy and bunker up to rebuild the VM")
}

fn set_immutable(vm_dir: &Path, on: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let yaml = vm_dir.join("lima.yaml");
    let Ok(m) = std::fs::symlink_metadata(&yaml) else {
        return Ok(());
    };
    if on && (!m.file_type().is_file() || m.nlink() != 1) {
        return Err(tampered(format!(
            "{} is linked or not a plain file; refusing to start from it",
            yaml.display()
        )));
    }
    let flags = std::os::macos::fs::MetadataExt::st_flags(&m);
    let new = if on {
        flags | libc::UF_IMMUTABLE
    } else {
        flags & !libc::UF_IMMUTABLE
    };
    let c = std::ffi::CString::new(yaml.as_os_str().as_encoded_bytes())
        .map_err(|_| tampered(format!("{} has a NUL in its path", yaml.display())))?;
    // SAFETY: lchflags on a NUL-terminated path; it never follows a final symlink.
    if unsafe { libc::lchflags(c.as_ptr(), new) } != 0 {
        return Err(BunkerError::io(
            format!("cannot change flags on {}", yaml.display()),
            std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

/// Refuses an instance whose lima.yaml is unsealed, linked or not a plain file.
pub fn check_sealed(vm_dir: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let yaml = vm_dir.join("lima.yaml");
    let Ok(m) = std::fs::symlink_metadata(&yaml) else {
        return Ok(());
    };
    let sealed = std::os::macos::fs::MetadataExt::st_flags(&m) & libc::UF_IMMUTABLE != 0;
    if !m.file_type().is_file() || m.nlink() != 1 || !sealed {
        return Err(tampered(format!(
            "{} is not sealed by bunker (unsealed, linked or replaced)",
            yaml.display()
        )));
    }
    Ok(())
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

#[cfg(test)]
mod seal_tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn unseal_for_cleanup(p: &Path) {
        let c = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
        unsafe { libc::chflags(c.as_ptr(), 0) };
    }

    #[test]
    fn sealed_lima_yaml_refuses_hard_links_and_is_checked() {
        let d = tempfile::tempdir().unwrap();
        let yaml = d.path().join("lima.yaml");
        std::fs::write(&yaml, "vmType: vz\n").unwrap();
        assert_eq!(
            check_sealed(d.path()).unwrap_err().code,
            "instance_tampered",
            "unsealed refuses"
        );
        seal_instance_yaml(d.path()).unwrap();
        check_sealed(d.path()).unwrap();
        assert!(
            std::fs::hard_link(&yaml, d.path().join("x")).is_err(),
            "hard link to a sealed file"
        );
        assert_eq!(std::fs::metadata(&yaml).unwrap().nlink(), 1);
        unseal_instance_yaml(d.path()).unwrap();
        std::fs::hard_link(&yaml, d.path().join("x")).unwrap();
        assert_eq!(
            seal_instance_yaml(d.path()).unwrap_err().code,
            "instance_tampered",
            "a linked yaml is never sealed"
        );
        unseal_for_cleanup(&yaml);
    }

    #[test]
    fn missing_lima_yaml_is_fine_to_seal() {
        let d = tempfile::tempdir().unwrap();
        seal_instance_yaml(d.path()).unwrap();
        unseal_instance_yaml(d.path()).unwrap();
    }
}
