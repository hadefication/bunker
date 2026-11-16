//! LaunchAgents: every long-running piece (proxy, each VM's hostagent, both forwards, cloudflared)
//! is a launchd job in the user's GUI domain.
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use crate::run::{Cmd, Output, Runner};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Proxy,
    Vm,
    FwdProxy,
    FwdWeb,
    Tunnel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub label: String,
    pub program_args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub keep_alive: bool,
    pub run_at_load: bool,
    pub throttle_secs: Option<u32>,
    pub stdout: PathBuf,
    pub stderr: PathBuf,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentState {
    pub loaded: bool,
    pub pid: Option<i32>,
    pub last_exit: Option<i32>,
}

/// `com.bunker.proxy`, or `com.bunker.<project>.{vm,fwd-proxy,fwd-web,tunnel}`.
pub fn label(project: Option<&str>, role: Role) -> String {
    let suffix = match role {
        Role::Proxy => return "com.bunker.proxy".to_string(),
        Role::Vm => "vm",
        Role::FwdProxy => "fwd-proxy",
        Role::FwdWeb => "fwd-web",
        Role::Tunnel => "tunnel",
    };
    format!("com.bunker.{}.{suffix}", project.unwrap_or("_"))
}

fn domain() -> String {
    // SAFETY: getuid has no preconditions and cannot fail.
    format!("gui/{}", unsafe { libc::getuid() })
}

fn failed(what: &str, label: &str, out: &Output) -> BunkerError {
    BunkerError::new(
        "launchd_failed",
        ExitCode::Failure,
        format!(
            "launchctl {what} {label} exited {}: {}",
            out.status,
            out.stderr_str().trim()
        ),
    )
}

/// `bootout` of a job that isn't loaded exits 3 ("No such process") or 113 ("Could not find service").
fn not_loaded(out: &Output) -> bool {
    matches!(out.status, 3 | 113)
        || out.stderr_str().contains("Could not find service")
        || out.stderr_str().contains("No such process")
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(*c as u32, 0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F))
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&apos;".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

/// `$BUNKER_LAUNCH_AGENTS_DIR`, else `~/Library/LaunchAgents`.
pub fn agents_dir() -> PathBuf {
    match std::env::var_os("BUNKER_LAUNCH_AGENTS_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Library/LaunchAgents")
        }
    }
}

pub fn render_plist(a: &Agent) -> String {
    let b = |v: bool| if v { "<true/>" } else { "<false/>" };
    let mut p = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    let _ = writeln!(
        p,
        "    <key>Label</key>\n    <string>{}</string>",
        xml_escape(&a.label)
    );
    p.push_str("    <key>ProgramArguments</key>\n    <array>\n");
    for arg in &a.program_args {
        let _ = writeln!(p, "        <string>{}</string>", xml_escape(arg));
    }
    p.push_str("    </array>\n");
    if !a.env.is_empty() {
        p.push_str("    <key>EnvironmentVariables</key>\n    <dict>\n");
        for (k, v) in &a.env {
            let _ = writeln!(
                p,
                "        <key>{}</key>\n        <string>{}</string>",
                xml_escape(k),
                xml_escape(v)
            );
        }
        p.push_str("    </dict>\n");
    }
    let _ = writeln!(p, "    <key>KeepAlive</key>\n    {}", b(a.keep_alive));
    let _ = writeln!(p, "    <key>RunAtLoad</key>\n    {}", b(a.run_at_load));
    if let Some(t) = a.throttle_secs {
        let _ = writeln!(
            p,
            "    <key>ThrottleInterval</key>\n    <integer>{t}</integer>"
        );
    }
    let _ = writeln!(
        p,
        "    <key>StandardOutPath</key>\n    <string>{}</string>",
        xml_escape(&a.stdout.to_string_lossy())
    );
    let _ = writeln!(
        p,
        "    <key>StandardErrorPath</key>\n    <string>{}</string>",
        xml_escape(&a.stderr.to_string_lossy())
    );
    p.push_str("    <key>ProcessType</key>\n    <string>Background</string>\n</dict>\n</plist>\n");
    p
}

pub fn install(r: &dyn Runner, a: &Agent) -> Result<()> {
    install_in(r, &agents_dir(), a)
}

/// Writes the plist (0644, as launchd expects), boots out any loaded copy, and bootstraps it.
/// Bootstrap is retried twice: right after a bootout launchd can briefly answer EIO.
pub fn install_in(r: &dyn Runner, dir: &Path, a: &Agent) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| BunkerError::io(format!("cannot create {}", dir.display()), e))?;
    let plist = dir.join(format!("{}.plist", a.label));
    crate::paths::write_atomic(&plist, render_plist(a).as_bytes(), 0o644)?;
    bootout(r, &a.label)?;
    let cmd = Cmd::new("launchctl").args([
        "bootstrap".to_string(),
        domain(),
        plist.to_string_lossy().into_owned(),
    ]);
    let mut last = Output::default();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        last = r.run(&cmd)?;
        if last.success() {
            return Ok(());
        }
    }
    Err(failed("bootstrap", &a.label, &last))
}

fn bootout(r: &dyn Runner, label: &str) -> Result<()> {
    let out = r.run(
        &Cmd::new("launchctl").args(["bootout".to_string(), format!("{}/{label}", domain())]),
    )?;
    if out.success() || not_loaded(&out) {
        Ok(())
    } else {
        Err(failed("bootout", label, &out))
    }
}

pub fn remove(r: &dyn Runner, label: &str) -> Result<()> {
    remove_in(r, &agents_dir(), label)
}

/// Boots the job out (not loaded is fine) and deletes its plist (missing is fine).
pub fn remove_in(r: &dyn Runner, dir: &Path, label: &str) -> Result<()> {
    bootout(r, label)?;
    let plist = dir.join(format!("{label}.plist"));
    match std::fs::remove_file(&plist) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(BunkerError::io(
            format!("cannot remove {}", plist.display()),
            e,
        )),
        _ => Ok(()),
    }
}

/// Restarts a loaded job (`kickstart -k`).
pub fn kickstart(r: &dyn Runner, label: &str) -> Result<()> {
    let out = r.run(&Cmd::new("launchctl").args([
        "kickstart".to_string(),
        "-k".into(),
        format!("{}/{label}", domain()),
    ]))?;
    if out.success() {
        Ok(())
    } else {
        Err(failed("kickstart", label, &out))
    }
}

/// Parsed from `launchctl print`; only the job's own (one-tab) properties count.
pub fn status(r: &dyn Runner, label: &str) -> Result<AgentState> {
    let out =
        r.run(&Cmd::new("launchctl").args(["print".to_string(), format!("{}/{label}", domain())]))?;
    if !out.success() {
        return if not_loaded(&out) {
            Ok(AgentState::default())
        } else {
            Err(failed("print", label, &out))
        };
    }
    let text = out.stdout_str();
    let prop = |key: &str| {
        text.lines()
            .filter(|l| l.starts_with('\t') && !l.starts_with("\t\t"))
            .find_map(|l| {
                l.trim()
                    .strip_prefix(key)?
                    .strip_prefix(" = ")
                    .map(str::to_string)
            })
    };
    Ok(AgentState {
        loaded: true,
        pid: prop("pid").and_then(|v| v.parse().ok()),
        last_exit: prop("last exit code").and_then(|v| v.parse().ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{Output, RecordingRunner};

    fn agent() -> Agent {
        Agent {
            label: label(Some("life-os"), Role::FwdWeb),
            program_args: vec!["/usr/bin/sandbox-exec".into(), "a&b<c>".into()],
            env: BTreeMap::from([("LIMA_HOME".into(), "/Users/x/.lima".into())]),
            keep_alive: true,
            run_at_load: true,
            throttle_secs: Some(5),
            stdout: "/tmp/out.log".into(),
            stderr: "/tmp/err.log".into(),
        }
    }

    fn uid() -> u32 {
        unsafe { libc::getuid() }
    }

    #[test]
    fn labels_follow_the_global_constraints() {
        assert_eq!(label(None, Role::Proxy), "com.bunker.proxy");
        assert_eq!(label(Some("life-os"), Role::Vm), "com.bunker.life-os.vm");
        assert_eq!(
            label(Some("life-os"), Role::FwdProxy),
            "com.bunker.life-os.fwd-proxy"
        );
        assert_eq!(
            label(Some("life-os"), Role::FwdWeb),
            "com.bunker.life-os.fwd-web"
        );
        assert_eq!(
            label(Some("life-os"), Role::Tunnel),
            "com.bunker.life-os.tunnel"
        );
    }

    #[test]
    fn plist_escapes_xml_and_lists_args() {
        let p = render_plist(&agent());
        assert!(p.contains("<string>a&amp;b&lt;c&gt;</string>"), "{p}");
        assert!(
            p.contains("<key>Label</key>\n    <string>com.bunker.life-os.fwd-web</string>"),
            "{p}"
        );
        assert!(p.contains("<key>KeepAlive</key>\n    <true/>"), "{p}");
        assert!(
            p.contains("<key>ThrottleInterval</key>\n    <integer>5</integer>"),
            "{p}"
        );
        assert!(
            p.contains("<key>LIMA_HOME</key>\n        <string>/Users/x/.lima</string>"),
            "{p}"
        );
        let mut a = agent();
        a.keep_alive = false;
        a.throttle_secs = None;
        a.env.clear();
        let p = render_plist(&a);
        assert!(
            !p.contains("ThrottleInterval") && !p.contains("EnvironmentVariables"),
            "{p}"
        );
        assert!(p.contains("<key>KeepAlive</key>\n    <false/>"), "{p}");
    }

    #[test]
    fn install_runs_bootout_then_bootstrap() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "launchctl",
            &["bootout"],
            Output::fail(3, "Boot-out failed: 3: No such process"),
        );
        install_in(&r, d.path(), &agent()).unwrap();
        let plist = d.path().join("com.bunker.life-os.fwd-web.plist");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&plist).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let calls: Vec<Vec<String>> = r.calls().into_iter().map(|c| c.args).collect();
        let domain = format!("gui/{}", uid());
        assert_eq!(
            calls,
            [
                vec![
                    "bootout".to_string(),
                    format!("{domain}/com.bunker.life-os.fwd-web")
                ],
                vec![
                    "bootstrap".to_string(),
                    domain,
                    plist.to_string_lossy().into_owned()
                ],
            ]
        );
    }

    #[test]
    fn install_surfaces_bootstrap_failure() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        for _ in 0..3 {
            r.respond(
                "launchctl",
                &["bootstrap"],
                Output::fail(5, "Bootstrap failed: 5: Input/output error"),
            );
        }
        let e = install_in(&r, d.path(), &agent()).unwrap_err();
        assert_eq!(e.code, "launchd_failed");
        assert!(e.message.contains("Input/output error"), "{}", e.message);
    }

    #[test]
    fn remove_tolerates_not_loaded() {
        let d = tempfile::tempdir().unwrap();
        for status in [3, 113] {
            let r = RecordingRunner::new();
            r.respond(
                "launchctl",
                &["bootout"],
                Output::fail(status, "Could not find service"),
            );
            remove_in(&r, d.path(), "com.bunker.x.vm").unwrap();
        }
        let r = RecordingRunner::new();
        r.respond(
            "launchctl",
            &["bootout"],
            Output::fail(1, "Operation not permitted"),
        );
        assert_eq!(
            remove_in(&r, d.path(), "com.bunker.x.vm").unwrap_err().code,
            "launchd_failed"
        );
        let plist = d.path().join("com.bunker.x.tunnel.plist");
        std::fs::write(&plist, "x").unwrap();
        remove_in(&RecordingRunner::new(), d.path(), "com.bunker.x.tunnel").unwrap();
        assert!(!plist.exists());
    }

    #[test]
    fn status_parses_pid_and_last_exit() {
        let r = RecordingRunner::new();
        let text = "gui/501/com.bunker.p.vm = {\n\tactive count = 1\n\tstate = running\n\n\tprogram = /usr/bin/sandbox-exec\n\tpid = 4242\n\tlast exit code = 0\n\tendpoints = {\n\t\tpid = 1\n\t}\n}\n";
        r.respond("launchctl", &["print"], Output::ok(text));
        assert_eq!(
            status(&r, "com.bunker.p.vm").unwrap(),
            AgentState {
                loaded: true,
                pid: Some(4242),
                last_exit: Some(0)
            }
        );
        r.respond(
            "launchctl",
            &["print"],
            Output::ok("x = {\n\tstate = not running\n\tlast exit code = (never exited)\n}\n"),
        );
        assert_eq!(
            status(&r, "com.bunker.p.vm").unwrap(),
            AgentState {
                loaded: true,
                pid: None,
                last_exit: None
            }
        );
        r.respond(
            "launchctl",
            &["print"],
            Output::fail(113, "Could not find service"),
        );
        assert_eq!(
            status(&r, "com.bunker.p.vm").unwrap(),
            AgentState::default()
        );
        assert_eq!(
            r.calls()[0].args,
            [
                "print".to_string(),
                format!("gui/{}/com.bunker.p.vm", uid())
            ]
        );
    }

    #[test]
    #[ignore]
    fn sys_plist_lints() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.plist");
        std::fs::write(&p, render_plist(&agent())).unwrap();
        let out = std::process::Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&p)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
