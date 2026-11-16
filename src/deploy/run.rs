//! Runs a deploy or a rollback against the project's guest.
use crate::commands::env::push_env;
use crate::config::Config;
use crate::config::detect::detect;
use crate::deploy::lock::acquire;
use crate::deploy::source::{build_archive, release_name};
use crate::deploy::steps::{
    As, BASE, is_release_name, releases_script, reload_script, steps, switch_script,
};
use crate::error::{BunkerError, Result};
use crate::host::Host;
use crate::lima::confine::Confinement;
use crate::lima::vm::{shell, vm_name};
use crate::output::{ExitCode, Report};
use crate::paths::{ProjectDir, ensure_private_dir, write_private};
use crate::proxy::lease::{LeaseGuard, LeaseKind, open_lease};
use crate::secrets::dotenv::render_env;
use crate::secrets::generate::generate_missing;
use crate::secrets::store::SecretStore;
use serde_json::{Value, json};
use std::io::{self, Write};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Copy, Default)]
pub struct DeployOpts {
    pub dirty: bool,
    pub allow_advisories: bool,
}

#[derive(Debug, Clone)]
pub struct StepResult {
    pub name: &'static str,
    pub ok: bool,
    pub secs: f64,
}

#[derive(Debug)]
pub struct DeployReport {
    pub project: String,
    pub release: String,
    pub sha: String,
    pub dirty: bool,
    pub first: bool,
    pub steps: Vec<StepResult>,
    pub url: Option<String>,
}

impl Report for DeployReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "{}: deployed {}", self.project, self.release)?;
        if let Some(u) = &self.url {
            writeln!(w, "  {u}")?;
        }
        Ok(())
    }
    fn json(&self) -> Value {
        json!({
            "project": self.project, "release": self.release, "sha": self.sha,
            "dirty": self.dirty, "first": self.first, "url": self.url,
            "steps": self.steps.iter().map(|s| json!({"name": s.name, "ok": s.ok, "secs": s.secs})).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug)]
pub struct RollbackReport {
    pub project: String,
    pub from: String,
    pub to: String,
}

pub const MIGRATIONS_NOTE: &str = "migrations are not rolled back";

impl Report for RollbackReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(
            w,
            "{}: rolled back {} -> {}",
            self.project, self.from, self.to
        )?;
        writeln!(w, "  note: {MIGRATIONS_NOTE}")
    }
    fn json(&self) -> Value {
        json!({"project": self.project, "from": self.from, "to": self.to, "note": MIGRATIONS_NOTE})
    }
}

/// The Build lease covers the package steps only (composer, npm and their audits).
const BUILD_LEASE: Duration = Duration::from_secs(45 * 60);

pub fn deploy(
    h: &Host,
    store: &dyn SecretStore,
    project: &str,
    opts: DeployOpts,
    progress: &dyn Fn(&StepResult),
) -> Result<DeployReport> {
    let pd = ProjectDir::under(&h.home, project);
    let cfg = Config::load(&pd)?;
    let _lock = acquire(&pd.deploy_lock())?;
    require_confined(h, project)?;
    let archive = build_archive(&cfg.project, opts.dirty)?;
    let release = release_name(SystemTime::now(), &archive.sha, archive.dirty);
    assert!(is_release_name(&release), "release_name made {release:?}");
    let vm = vm_name(project);
    let first =
        !h.r.run(&shell(h, &vm).args(["test", "-e", "/srv/app/shared/.env"]))?
            .success();
    let dir = format!("{BASE}/releases/{release}");
    let mut log = Log::new(&pd)?;
    let mut results = Vec::new();
    let mut lease: Option<LeaseGuard> = None;
    let mut switched = false;
    for st in steps(&cfg, &release, first, opts.allow_advisories) {
        if st.needs_lease && lease.is_none() {
            lease = Some(open_lease(&pd, LeaseKind::Build, BUILD_LEASE)?);
        } else if !st.needs_lease {
            lease = None;
        }
        let start = Instant::now();
        log.heading(st.name);
        let res = match st.name {
            "upload" => upload(h, &vm, &dir, &archive.tar),
            "write_env" => write_env(h, store, &pd),
            _ => run_script(h, &vm, st.run_as, &st.cmd, &mut log),
        };
        let r = StepResult {
            name: st.name,
            ok: res.is_ok(),
            secs: start.elapsed().as_secs_f64(),
        };
        progress(&r);
        results.push(r);
        if let Err(e) = res {
            drop(lease.take());
            if !switched {
                // A half-built release is never left behind; current is untouched.
                let rm = format!("rm -rf -- {dir}\n");
                let _ = run_script(h, &vm, As::Root, &rm, &mut log);
            }
            log.save();
            return Err(step_error(&pd, st.name, switched, &release, e));
        }
        if st.name == "switch" {
            switched = true;
        }
    }
    drop(lease);
    log.save();
    Ok(DeployReport {
        project: project.to_string(),
        release,
        sha: archive.sha,
        dirty: archive.dirty,
        first,
        steps: results,
        url: Some(match &cfg.domain {
            Some(d) => format!("https://{d}"),
            None => format!("http://127.0.0.1:{}", cfg.ports.web),
        }),
    })
}

pub fn rollback(h: &Host, project: &str) -> Result<RollbackReport> {
    let pd = ProjectDir::under(&h.home, project);
    let cfg = Config::load(&pd)?;
    let _lock = acquire(&pd.deploy_lock())?;
    require_confined(h, project)?;
    let vm = vm_name(project);
    let mut log = Log::new(&pd)?;
    let out = run_out(h, &vm, As::Root, &releases_script(BASE), &mut log)?;
    let field = |k: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .unwrap_or("")
            .to_string()
    };
    let (from, to) = (field("current"), field("previous"));
    for name in [&from, &to] {
        if !name.is_empty() && !is_release_name(name) {
            return Err(BunkerError::new(
                "bad_release_name",
                ExitCode::Refused,
                format!(
                    "{project}: {name:?} in /srv/app is not a bunker release name; refusing to use it"
                ),
            ));
        }
    }
    if to.is_empty() {
        return Err(BunkerError::new(
            "no_previous_release",
            ExitCode::Refused,
            format!("{project} has no release before {from} to roll back to"),
        ));
    }
    run_script(h, &vm, As::Root, &switch_script(BASE, &to), &mut log)?;
    run_script(h, &vm, As::Root, &reload_script(&cfg), &mut log)?;
    Ok(RollbackReport {
        project: project.to_string(),
        from,
        to,
    })
}

fn require_confined(h: &Host, project: &str) -> Result<()> {
    match h.probe.confinement(project)? {
        Confinement::Confined { .. } => Ok(()),
        Confinement::Unconfined { reason, .. } => Err(BunkerError::new(
            "unconfined",
            ExitCode::Unconfined,
            format!("{project}'s VM is not confined ({reason}); refusing to deploy into it"),
        )
        .hint(format!("bunker restart {project}"))),
        Confinement::Stopped => Err(BunkerError::new(
            "vm_not_running",
            ExitCode::Failure,
            format!("{project}'s VM is not running"),
        )
        .hint(format!("bunker start {project}"))),
    }
}

/// `mkdir` the release as deploy, then unpack the tar from stdin into it.
fn upload(h: &Host, vm: &str, dir: &str, tar: &[u8]) -> Result<()> {
    h.r.run_ok(&shell(h, vm).args(["sudo", "-u", "deploy", "mkdir", "-p", dir]))?;
    h.r.run_ok(
        &shell(h, vm)
            .args(["sudo", "-u", "deploy", "tar", "-x", "-C", dir, "-f", "-"])
            .stdin(tar.to_vec()),
    )?;
    Ok(())
}

/// First deploy: generate missing secrets, then install the rendered .env from stdin.
fn write_env(h: &Host, store: &dyn SecretStore, pd: &ProjectDir) -> Result<()> {
    let cfg = Config::load(pd)?;
    let passport = detect(&cfg.project)?.passport;
    generate_missing(pd, &cfg, passport, store, h.r)?;
    let cfg = Config::load(pd)?;
    let rendered = render_env(&pd.name, &cfg, store)?;
    push_env(h, &pd.name, &rendered)?;
    Ok(())
}

fn as_args(who: As) -> &'static [&'static str] {
    match who {
        As::Deploy => &["sudo", "-u", "deploy", "-H", "bash", "-s"],
        As::App => &["sudo", "-u", "app", "bash", "-s"],
        As::Root => &["sudo", "bash", "-s"],
    }
}

/// Runs a script on stdin as `who`; its output goes to the deploy log.
fn run_out(h: &Host, vm: &str, who: As, script: &str, log: &mut Log) -> Result<String> {
    let out = h.r.run(
        &shell(h, vm)
            .args(as_args(who).iter().copied())
            .stdin(script.as_bytes().to_vec()),
    )?;
    log.add(&out.stdout);
    log.add(&out.stderr);
    if !out.success() {
        let text = if out.stderr.is_empty() {
            out.stdout_str()
        } else {
            out.stderr_str()
        };
        let lines: Vec<&str> = text.lines().collect();
        return Err(BunkerError::new(
            "command_failed",
            ExitCode::Failure,
            format!(
                "exit {}: {}",
                out.status,
                lines[lines.len().saturating_sub(5)..].join("\n")
            ),
        ));
    }
    Ok(out.stdout_str())
}

fn run_script(h: &Host, vm: &str, who: As, script: &str, log: &mut Log) -> Result<()> {
    run_out(h, vm, who, script, log).map(|_| ())
}

fn step_error(
    pd: &ProjectDir,
    step: &str,
    switched: bool,
    release: &str,
    e: BunkerError,
) -> BunkerError {
    let log = pd.logs_dir().join("deploy.log");
    if step.ends_with("_audit") {
        return BunkerError::new(
            "advisories",
            ExitCode::Refused,
            format!("{step} reported security advisories; nothing was switched"),
        )
        .hint(format!(
            "details in {}; update the packages, or deploy anyway with --allow-advisories",
            log.display()
        ));
    }
    let state = if switched {
        format!("{release} is live; this step ran after the switch")
    } else {
        "nothing was switched; the half-built release was removed".to_string()
    };
    BunkerError::new(
        "step_failed",
        ExitCode::Failure,
        format!("{step} failed ({state}): {}", e.message),
    )
    .hint(format!(
        "full output in {}; blocked hosts: bunker net log {} --blocked",
        log.display(),
        pd.name
    ))
}

/// Everything the guest printed, saved to `logs/deploy.log` (0600) when the run ends.
struct Log {
    path: std::path::PathBuf,
    buf: Vec<u8>,
}

impl Log {
    fn new(pd: &ProjectDir) -> Result<Self> {
        ensure_private_dir(&pd.logs_dir())?;
        Ok(Self {
            path: pd.logs_dir().join("deploy.log"),
            buf: Vec::new(),
        })
    }
    fn heading(&mut self, step: &str) {
        self.buf
            .extend_from_slice(format!("== {step}\n").as_bytes());
    }
    fn add(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }
    fn save(&self) {
        let _ = write_private(&self.path, &self.buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, FakeProbe, host, project};
    use crate::lima::confine::Confinement;
    use crate::paths::ProjectDir;
    use crate::proxy::lease::{LeaseKind, lease_active};
    use crate::run::{Cmd, Output, RecordingRunner};
    use crate::secrets::store::MemoryStore;
    use std::process::Command;

    const SHELL: [&str; 5] = ["shell", "--tty=false", "--workdir", "/", "bunker-t"];

    fn stdin(c: &Cmd) -> String {
        c.stdin
            .as_deref()
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .unwrap_or_default()
    }

    /// A git repo for the project to deploy, and the config pointed at it.
    fn fixture(h: &Host) -> (ProjectDir, tempfile::TempDir) {
        let pd = project(h, "t");
        let repo = tempfile::tempdir().unwrap();
        let p = repo.path();
        std::fs::write(p.join("artisan"), "").unwrap();
        std::fs::write(
            p.join("composer.json"),
            r#"{"require":{"php":"^8.3","laravel/framework":"^12.0"}}"#,
        )
        .unwrap();
        for args in [
            &["init", "-q"][..],
            &["add", "."],
            &["commit", "-qm", "init"],
        ] {
            assert!(
                Command::new("git")
                    .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
                    .args(args)
                    .current_dir(p)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let text = std::fs::read_to_string(pd.config()).unwrap();
        let text = text.replace(
            &format!("\"{}\"", h.home.display()),
            &format!("\"{}\"", p.display()),
        );
        std::fs::write(pd.config(), text).unwrap();
        (pd, repo)
    }

    fn runner(first: bool) -> RecordingRunner {
        let r = RecordingRunner::new();
        r.respond(
            "limactl",
            &["list"],
            Output::ok("{\"name\":\"bunker-t\",\"status\":\"Running\",\"dir\":\"/x\"}\n"),
        );
        let mut probe: Vec<&str> = SHELL.to_vec();
        probe.extend(["test", "-e", "/srv/app/shared/.env"]);
        r.respond(
            "limactl",
            &probe,
            if first {
                Output::fail(1, "")
            } else {
                Output::ok("")
            },
        );
        r
    }

    fn quiet(_: &StepResult) {}

    /// Answers every guest shell call with `f` (RecordingRunner expectations are one-shot).
    fn on_shell(r: &RecordingRunner, f: impl Fn(&Cmd) -> Output + Send + Sync + 'static) {
        let f = std::sync::Arc::new(f);
        for _ in 0..40 {
            let f = f.clone();
            r.respond_with("limactl", &SHELL, move |c| f(c));
        }
    }

    #[test]
    fn deploy_refuses_when_unconfined() {
        let d = tempfile::tempdir().unwrap();
        let r = runner(false);
        let probe = FakeProbe(Confinement::Unconfined {
            hostagent: 1,
            reason: "x".into(),
        });
        let h = host(&r, &probe, d.path());
        let _f = fixture(&h);
        let e = deploy(
            &h,
            &MemoryStore::default(),
            "t",
            DeployOpts::default(),
            &quiet,
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("unconfined", crate::output::ExitCode::Unconfined)
        );
        assert!(
            !r.calls().iter().any(|c| c.args.iter().any(|a| a == "tar")),
            "no upload"
        );
    }

    #[test]
    fn build_lease_closed_after_failed_composer() {
        let d = tempfile::tempdir().unwrap();
        let r = runner(false);
        on_shell(&r, |c| {
            if stdin(c).contains("composer install") {
                Output::fail(2, "composer exploded")
            } else {
                Output::ok("")
            }
        });
        let h = host(&r, &CONFINED, d.path());
        let (pd, _repo) = fixture(&h);
        let e = deploy(
            &h,
            &MemoryStore::default(),
            "t",
            DeployOpts::default(),
            &quiet,
        )
        .unwrap_err();
        assert_eq!(e.code, "step_failed");
        assert!(e.message.contains("composer_install"), "{}", e.message);
        assert!(!lease_active(
            &pd,
            LeaseKind::Build,
            std::time::SystemTime::now()
        ));
        let calls = r.calls();
        assert!(
            !calls.iter().any(|c| stdin(c).contains("mv -T")),
            "never switched"
        );
        assert!(
            calls
                .iter()
                .any(|c| stdin(c).contains("rm -rf -- /srv/app/releases/")),
            "half-built release removed"
        );
    }

    #[test]
    fn failed_audit_is_refused_as_advisories() {
        let d = tempfile::tempdir().unwrap();
        let r = runner(false);
        on_shell(&r, |c| {
            if stdin(c).contains("composer audit") {
                Output::fail(1, "1 advisory")
            } else {
                Output::ok("")
            }
        });
        let h = host(&r, &CONFINED, d.path());
        let _f = fixture(&h);
        let e = deploy(
            &h,
            &MemoryStore::default(),
            "t",
            DeployOpts::default(),
            &quiet,
        )
        .unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("advisories", crate::output::ExitCode::Refused)
        );
    }

    #[test]
    fn later_deploy_switches_and_reports_every_step() {
        let d = tempfile::tempdir().unwrap();
        let r = runner(false);
        let h = host(&r, &CONFINED, d.path());
        let _f = fixture(&h);
        let seen = std::sync::Mutex::new(Vec::new());
        let rep = deploy(
            &h,
            &MemoryStore::default(),
            "t",
            DeployOpts::default(),
            &|s| seen.lock().unwrap().push(s.name),
        )
        .unwrap();
        assert!(!rep.first);
        assert_eq!(
            *seen.lock().unwrap(),
            rep.steps.iter().map(|s| s.name).collect::<Vec<_>>()
        );
        assert!(rep.steps.iter().all(|s| s.ok));
        let calls = r.calls();
        let tar = calls
            .iter()
            .find(|c| c.args.iter().any(|a| a == "tar"))
            .expect("upload");
        assert!(
            tar.args
                .contains(&format!("/srv/app/releases/{}", rep.release)),
            "{:?}",
            tar.args
        );
        assert!(calls.iter().any(|c| stdin(c).contains("mv -T")));
        assert_eq!(rep.url.as_deref(), Some("https://life-os.bangkila.com"));
    }

    #[test]
    fn first_deploy_env_on_stdin_only() {
        let d = tempfile::tempdir().unwrap();
        let r = runner(true);
        let h = host(&r, &CONFINED, d.path());
        let (pd, _repo) = fixture(&h);
        let store = MemoryStore::default();
        let rep = deploy(&h, &store, "t", DeployOpts::default(), &quiet).unwrap();
        assert!(rep.first);
        assert!(rep.steps.iter().any(|s| s.name == "write_env"));
        let cfg = crate::config::Config::load(&pd).unwrap();
        let mut secrets = Vec::new();
        for k in cfg.secrets.keys() {
            secrets.push(String::from_utf8(store.get("t", k).unwrap().unwrap().to_vec()).unwrap());
        }
        assert!(secrets.len() >= 2, "APP_KEY and DB_PASSWORD generated");
        r.assert_no_secret_in_argv(&secrets.iter().map(String::as_str).collect::<Vec<_>>());
        let env = r
            .calls()
            .into_iter()
            .find(|c| {
                c.args.iter().any(|a| a == "/srv/app/shared/.env")
                    && c.args.iter().any(|a| a == "install")
            })
            .expect(".env install");
        assert!(stdin(&env).contains("APP_KEY="));
    }

    #[test]
    fn rollback_without_previous_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "limactl",
            &SHELL,
            Output::ok("current=20261116T000001Z-aaaaaaa\nprevious=\n"),
        );
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let e = rollback(&h, "t").unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("no_previous_release", crate::output::ExitCode::Refused)
        );
        assert!(!r.calls().iter().any(|c| stdin(c).contains("mv -T")));
    }

    #[test]
    fn rollback_refuses_a_name_that_is_not_a_release() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "limactl",
            &SHELL,
            Output::ok("current=20261116T000003Z-aaaaaaa\nprevious=x; id\n"),
        );
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let e = rollback(&h, "t").unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("bad_release_name", crate::output::ExitCode::Refused)
        );
        assert!(!r.calls().iter().any(|c| stdin(c).contains("current.tmp")));
    }

    #[test]
    fn rollback_says_migrations_not_undone() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond_with("limactl", &SHELL, |c| {
            if stdin(c).contains("previous=") {
                Output::ok("current=20261116T000002Z-aaaaaaa\nprevious=20261116T000001Z-aaaaaaa\n")
            } else {
                Output::ok("")
            }
        });
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let rep = rollback(&h, "t").unwrap();
        assert_eq!(
            (rep.from.as_str(), rep.to.as_str()),
            ("20261116T000002Z-aaaaaaa", "20261116T000001Z-aaaaaaa")
        );
        let mut out = Vec::new();
        rep.human(&mut out).unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("migrations are not rolled back")
        );
        assert_eq!(rep.json()["note"], "migrations are not rolled back");
        assert!(r.calls().iter().any(|c| {
            stdin(c).contains("ln -sfn releases/20261116T000001Z-aaaaaaa /srv/app/current.tmp")
        }));
    }
}
