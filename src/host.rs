//! Everything a command needs from the machine, in one place, so tests swap in a recording runner,
//! a fake confinement probe and temp directories.
use crate::error::{BunkerError, Result};
use crate::lima::confine::ConfinementProbe;
use crate::output::ExitCode;
use crate::run::Runner;
use std::path::PathBuf;
use std::time::Duration;

pub struct Host<'a> {
    pub r: &'a dyn Runner,
    pub probe: &'a dyn ConfinementProbe,
    /// `~/.bunker` (or `BUNKER_HOME`).
    pub home: PathBuf,
    /// `~/Library/LaunchAgents` (or `BUNKER_LAUNCH_AGENTS_DIR`).
    pub agents_dir: PathBuf,
    /// `$LIMA_HOME`, else `~/.lima`.
    pub lima_home: PathBuf,
    /// Absolute path of `limactl` (launchd needs one).
    pub limactl: PathBuf,
    /// This binary, for the proxy agent.
    pub bunker_exe: PathBuf,
    /// cloudflared (needed only for a project with a domain), and the account certificate
    /// `cloudflared tunnel login` writes to `~/.cloudflared/cert.pem`.
    pub cloudflared: PathBuf,
    pub cloudflared_cert: PathBuf,
    /// Pause between readiness checks, and how long to wait for a VM.
    pub poll: Duration,
    pub wait_timeout: Duration,
}

impl<'a> Host<'a> {
    pub fn system(r: &'a dyn Runner, probe: &'a dyn ConfinementProbe) -> Result<Host<'a>> {
        let limactl = find_in_path("limactl").ok_or_else(|| {
            BunkerError::new(
                "lima_missing",
                ExitCode::Failure,
                "limactl is not installed",
            )
            .hint("brew install lima")
        })?;
        Ok(Host {
            r,
            probe,
            home: crate::paths::bunker_home(),
            agents_dir: crate::launchd::agents_dir(),
            lima_home: lima_home(),
            limactl,
            bunker_exe: std::env::current_exe()
                .map_err(|e| BunkerError::io("cannot find the bunker binary", e))?,
            cloudflared: find_in_path("cloudflared").unwrap_or_else(|| "cloudflared".into()),
            cloudflared_cert: crate::secrets::keychain::user_home()
                .map(|h| h.join(".cloudflared/cert.pem"))
                .unwrap_or_default(),
            poll: Duration::from_secs(2),
            wait_timeout: Duration::from_secs(180),
        })
    }

    pub fn limactl(&self) -> String {
        self.limactl.to_string_lossy().into_owned()
    }
}

pub fn lima_home() -> PathBuf {
    match std::env::var_os("LIMA_HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".lima"),
    }
}

pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into()])
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use crate::lima::confine::{Confinement, ConfinementProbe};

    pub struct FakeProbe(pub Confinement);

    impl ConfinementProbe for FakeProbe {
        fn confinement(&self, _project: &str) -> Result<Confinement> {
            Ok(self.0.clone())
        }
        fn sandboxed(&self, _pid: i32) -> bool {
            matches!(self.0, Confinement::Confined { .. })
        }
    }

    pub const CONFINED: FakeProbe = FakeProbe(Confinement::Confined { hostagent: 4242 });

    /// A Host rooted in `dir`: home, LaunchAgents and LIMA_HOME are subdirectories; no waiting.
    pub fn host<'a>(
        r: &'a dyn Runner,
        probe: &'a dyn ConfinementProbe,
        dir: &std::path::Path,
    ) -> Host<'a> {
        Host {
            r,
            probe,
            home: dir.join("home"),
            agents_dir: dir.join("agents"),
            lima_home: dir.join("lima"),
            limactl: "limactl".into(),
            bunker_exe: "/usr/local/bin/bunker".into(),
            cloudflared: "cloudflared".into(),
            cloudflared_cert: dir.join("cert.pem"),
            poll: Duration::from_millis(1),
            wait_timeout: Duration::from_millis(50),
        }
    }

    /// Writes the fixture config for `project` under the host's home, with `project` = `dir`.
    pub fn project(h: &Host, name: &str) -> crate::paths::ProjectDir {
        let pd = crate::paths::ProjectDir::under(&h.home, name);
        std::fs::create_dir_all(&pd.root).unwrap();
        std::fs::create_dir_all(&h.lima_home).unwrap();
        let cfg = crate::config::fixture_config()
            .replace("/Users/inggo/Code/life-os", &h.home.to_string_lossy());
        std::fs::write(pd.config(), cfg).unwrap();
        // The repo the fixture points at: Laravel-shaped, so detect() accepts it.
        std::fs::write(h.home.join("artisan"), "").unwrap();
        std::fs::write(
            h.home.join("composer.json"),
            r#"{"require":{"php":"^8.3","laravel/framework":"^12.0"}}"#,
        )
        .unwrap();
        pd
    }
}
