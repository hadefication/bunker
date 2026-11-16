//! cloudflared, run through the Runner: find, create, route and delete named tunnels. Every call
//! names the account certificate explicitly.
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use crate::run::{Cmd, Output, Runner};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub struct Cloudflared<'a> {
    pub r: &'a dyn Runner,
    pub bin: PathBuf,
    /// `~/.cloudflared/cert.pem`, written by `cloudflared tunnel login`.
    pub cert: PathBuf,
}

#[derive(Deserialize)]
struct Tunnel {
    id: String,
    name: String,
}

impl Cloudflared<'_> {
    fn cmd(&self, args: &[&str]) -> Cmd {
        Cmd::new(self.bin.to_string_lossy())
            .args(["tunnel", "--origincert"])
            .arg(self.cert.to_string_lossy())
            .args(args.iter().copied())
    }

    fn run_ok(&self, args: &[&str]) -> Result<Output> {
        let out = self.r.run(&self.cmd(args))?;
        if out.success() {
            return Ok(out);
        }
        Err(BunkerError::new(
            "cloudflared_failed",
            ExitCode::Failure,
            format!(
                "cloudflared tunnel {} failed: {}",
                args.first().unwrap_or(&""),
                combined(&out).trim()
            ),
        ))
    }

    /// The id of the live tunnel called `name`. cloudflared may log before its JSON, so only the
    /// text from the first `[` to the last `]` is parsed.
    pub fn find(&self, name: &str) -> Result<Option<String>> {
        let out = self.run_ok(&["list", "--name", name, "-o", "json"])?;
        let text = out.stdout_str();
        let (Some(a), Some(b)) = (text.find('['), text.rfind(']')) else {
            return Ok(None);
        };
        let tunnels: Vec<Tunnel> = serde_json::from_str(&text[a..=b]).map_err(|e| {
            BunkerError::new(
                "cloudflared_failed",
                ExitCode::Failure,
                format!("cannot read cloudflared's tunnel list: {e}"),
            )
        })?;
        Ok(tunnels.into_iter().find(|t| t.name == name).map(|t| t.id))
    }

    /// Creates the tunnel, its credentials written to `creds` and made 0600.
    pub fn create(&self, name: &str, creds: &Path) -> Result<()> {
        let c = creds.to_string_lossy();
        self.run_ok(&["create", "--credentials-file", &c, name])?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(creds, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| BunkerError::io(format!("cannot chmod {}", creds.display()), e))
    }

    /// Points `host` at the tunnel with a CNAME. An existing record is refused unless `overwrite`.
    pub fn route(&self, id: &str, host: &str, overwrite: bool) -> Result<()> {
        let mut args = vec!["route", "dns"];
        if overwrite {
            args.push("-f");
        }
        args.extend([id, host]);
        let out = self.r.run(&self.cmd(&args))?;
        let text = combined(&out);
        if text.contains("already exists") && !overwrite {
            return Err(BunkerError::new(
                "dns_exists",
                ExitCode::Refused,
                format!("{host} already has a DNS record"),
            )
            .hint(
                "replace it with bunker up <project> --overwrite-dns (the old target stops getting traffic)",
            ));
        }
        if !out.success() {
            return Err(BunkerError::new(
                "cloudflared_failed",
                ExitCode::Failure,
                format!("cloudflared tunnel route dns failed: {}", text.trim()),
            ));
        }
        Ok(())
    }

    /// Drops the tunnel's connections, then deletes it. DNS records stay (cloudflared can't).
    pub fn delete(&self, id: &str) -> Result<()> {
        self.run_ok(&["cleanup", id])?;
        self.run_ok(&["delete", id])?;
        Ok(())
    }
}

fn combined(out: &Output) -> String {
    format!("{}{}", out.stdout_str(), out.stderr_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{Cmd, Output, RecordingRunner};
    use std::os::unix::fs::PermissionsExt;

    fn cf(r: &RecordingRunner) -> Cloudflared<'_> {
        Cloudflared {
            r,
            bin: "cloudflared".into(),
            cert: "/h/.cloudflared/cert.pem".into(),
        }
    }

    #[test]
    fn find_parses_the_json_between_the_brackets() {
        let r = RecordingRunner::new();
        r.respond(
            "cloudflared",
            &[
                "tunnel",
                "--origincert",
                "/h/.cloudflared/cert.pem",
                "list",
                "--name",
                "life-os",
                "-o",
                "json",
            ],
            Output::ok("2026-11-16T00:00:00Z INF note\n[{\"id\":\"6ff4\",\"name\":\"life-os\"}]\n"),
        );
        assert_eq!(cf(&r).find("life-os").unwrap().as_deref(), Some("6ff4"));
        let r = RecordingRunner::new();
        r.respond("cloudflared", &["tunnel"], Output::ok("[]\n"));
        assert_eq!(cf(&r).find("life-os").unwrap(), None);
    }

    #[test]
    fn route_existing_without_overwrite_is_refused_with_hint() {
        let r = RecordingRunner::new();
        r.respond(
            "cloudflared",
            &["tunnel"],
            Output::fail(1, "Failed to add route: code: 1003, reason: An A, AAAA, or CNAME record with that host already exists."),
        );
        let e = cf(&r)
            .route("6ff4", "life-os.bangkila.com", false)
            .unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("dns_exists", crate::output::ExitCode::Refused)
        );
        assert!(e.hint.unwrap().contains("--overwrite-dns"));
        assert!(!r.calls()[0].args.contains(&"-f".to_string()));
    }

    #[test]
    fn route_with_overwrite_passes_dash_f() {
        let r = RecordingRunner::new();
        cf(&r).route("6ff4", "life-os.bangkila.com", true).unwrap();
        assert_eq!(
            r.calls()[0].args,
            [
                "tunnel",
                "--origincert",
                "/h/.cloudflared/cert.pem",
                "route",
                "dns",
                "-f",
                "6ff4",
                "life-os.bangkila.com"
            ]
        );
    }

    #[test]
    fn credentials_written_0600() {
        let d = tempfile::tempdir().unwrap();
        let creds = d.path().join("tunnel-credentials.json");
        let r = RecordingRunner::new();
        r.respond_with("cloudflared", &["tunnel"], |c: &Cmd| {
            let i = c
                .args
                .iter()
                .position(|a| a == "--credentials-file")
                .unwrap();
            std::fs::write(&c.args[i + 1], "{}").unwrap();
            std::fs::set_permissions(&c.args[i + 1], std::fs::Permissions::from_mode(0o644))
                .unwrap();
            Output::ok("Created tunnel life-os with id 6ff4\n")
        });
        cf(&r).create("life-os", &creds).unwrap();
        assert_eq!(
            std::fs::metadata(&creds).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            r.calls()[0].args[3..],
            [
                "create",
                "--credentials-file",
                creds.to_str().unwrap(),
                "life-os"
            ]
        );
    }

    #[test]
    fn delete_cleans_up_connections_first() {
        let r = RecordingRunner::new();
        cf(&r).delete("6ff4").unwrap();
        let subs: Vec<String> = r.calls().iter().map(|c| c.args[3].clone()).collect();
        assert_eq!(subs, ["cleanup", "delete"]);
    }
}
