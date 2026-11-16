//! `bunker logs [project] [source] [-f] [-n N]`: guest logs through limactl, host logs from
//! `~/.bunker`, every line scrubbed of the project's secrets first.
use crate::config::Config;
use crate::error::{BunkerError, Result};
use crate::host::Host;
use crate::lima::vm::{shell, vm_name};
use crate::logs::scrub::Scrubber;
use crate::logs::tail::{follow, last_lines};
use crate::output::ExitCode;
use crate::paths::ProjectDir;
use crate::secrets::store::SecretStore;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Nginx,
    Php,
    Worker,
    App,
    Proxy,
    Tunnel,
}

pub const SOURCES: &str = "nginx, php, worker, app, proxy, tunnel";

impl Source {
    pub fn parse(s: &str) -> Option<Source> {
        Some(match s {
            "nginx" => Source::Nginx,
            "php" => Source::Php,
            "worker" => Source::Worker,
            "app" => Source::App,
            "proxy" => Source::Proxy,
            "tunnel" => Source::Tunnel,
            _ => return None,
        })
    }
}

/// `[project] [source]`: one word is a source when it names one and no project has that name.
pub fn parse_args(home: &std::path::Path, args: &[String]) -> Result<(Option<String>, Source)> {
    let bad = |s: &str| {
        BunkerError::new(
            "invalid_value",
            ExitCode::Usage,
            format!("unknown log source {s:?}: one of {SOURCES}"),
        )
    };
    match args {
        [] => Ok((None, Source::App)),
        [one] => match Source::parse(one) {
            Some(src) if !ProjectDir::under(home, one).config().exists() => Ok((None, src)),
            _ => Ok((Some(one.clone()), Source::App)),
        },
        [p, s] => Ok((Some(p.clone()), Source::parse(s).ok_or_else(|| bad(s))?)),
        _ => Err(BunkerError::new(
            "usage",
            ExitCode::Usage,
            "bunker logs [project] [source]",
        )),
    }
}

/// `--json` without `-f`: the lines, scrubbed.
pub struct LinesReport {
    pub lines: Vec<String>,
}

impl crate::output::Report for LinesReport {
    fn human(&self, w: &mut dyn std::io::Write) -> std::io::Result<()> {
        for l in &self.lines {
            writeln!(w, "{l}")?;
        }
        Ok(())
    }
    fn json(&self) -> serde_json::Value {
        serde_json::json!({"lines": self.lines})
    }
}

/// The guest script for a guest source; `None` for a host one.
fn guest_script(cfg: &Config, source: Source, follow: bool, n: usize) -> Option<String> {
    let f = if follow { " -F" } else { "" };
    let files = match source {
        Source::Nginx => "/var/log/nginx/access.log /var/log/nginx/error.log".to_string(),
        Source::Php => format!("/var/log/php{}-fpm.log", cfg.php),
        // Laravel's daily channel: the newest file.
        Source::App => {
            "\"$(ls -1t /srv/app/shared/storage/logs/*.log 2>/dev/null | head -n 1)\"".into()
        }
        Source::Worker => {
            let follow = if follow { " -f" } else { "" };
            return Some(format!(
                "exec journalctl --no-pager -o short-iso -n {n}{follow} -u 'bunker-worker-*'\n"
            ));
        }
        Source::Proxy | Source::Tunnel => return None,
    };
    Some(format!("exec tail -n {n}{f} {files}\n"))
}

pub fn logs(
    h: &Host,
    store: &dyn SecretStore,
    project: &str,
    source: Source,
    follow_on: bool,
    n: usize,
    out: &mut dyn FnMut(&str),
) -> Result<()> {
    let pd = ProjectDir::under(&h.home, project);
    let cfg = Config::load(&pd)?;
    let mut known = Vec::new();
    for key in cfg.secrets.keys() {
        if let Some(v) = store.get(project, key)? {
            known.push((
                key.clone(),
                Zeroizing::new(String::from_utf8_lossy(&v).into_owned()),
            ));
        }
    }
    let scrubber = Scrubber::new(known);
    let mut emit = |l: &str| out(&crate::output::printable(&scrubber.scrub(l)));
    if let Some(script) = guest_script(&cfg, source, follow_on, n) {
        let cmd = shell(h, &vm_name(project))
            .args(["sudo", "bash", "-s"])
            .stdin(script.into_bytes());
        let status = h.r.stream(&cmd, &mut emit)?;
        if status != 0 && !follow_on {
            return Err(BunkerError::new(
                "logs_failed",
                ExitCode::Failure,
                format!("reading the {source:?} logs in the guest exited {status}"),
            ));
        }
        return Ok(());
    }
    let path = match source {
        Source::Proxy => h.home.join("logs/proxy.log"),
        _ => pd.logs_dir().join("tunnel.err.log"),
    };
    for l in last_lines(&path, n)? {
        emit(&l);
    }
    if follow_on {
        follow(&path, &mut emit)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, host, project};
    use crate::run::{Output, RecordingRunner};
    use crate::secrets::store::MemoryStore;

    #[test]
    fn guest_lines_are_scrubbed_with_the_projects_secrets() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "limactl",
            &["shell"],
            Output::ok("SQLSTATE denied for app using Zx81LqTrw0Q\nok line\n"),
        );
        let h = host(&r, &CONFINED, d.path());
        let pd = project(&h, "t");
        let store = MemoryStore::default();
        crate::secrets::meta::record(
            &pd,
            "DB_PASSWORD",
            crate::secrets::meta::Entry::Secret(crate::config::SecretKind::InApp),
        )
        .unwrap();
        store.set("t", "DB_PASSWORD", b"Zx81LqTrw0Q").unwrap();
        let mut got = Vec::new();
        logs(&h, &store, "t", Source::App, false, 50, &mut |l| {
            got.push(l.to_string())
        })
        .unwrap();
        assert_eq!(
            got,
            [
                "SQLSTATE denied for app using [redacted:DB_PASSWORD]",
                "ok line"
            ]
        );
        let script = String::from_utf8(r.calls()[0].stdin.clone().unwrap()).unwrap();
        assert!(
            script.contains("tail -n 50") && script.contains("/srv/app/shared/storage/logs"),
            "{script}"
        );
    }

    #[test]
    fn host_sources_read_files_under_bunker_home() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        let pd = project(&h, "t");
        std::fs::create_dir_all(pd.logs_dir()).unwrap();
        std::fs::write(
            pd.logs_dir().join("tunnel.err.log"),
            "a\nb Authorization: Bearer x\nc\n",
        )
        .unwrap();
        let mut got = Vec::new();
        logs(
            &h,
            &MemoryStore::default(),
            "t",
            Source::Tunnel,
            false,
            2,
            &mut |l| got.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(got, ["b Authorization: [redacted]", "c"]);
        assert!(r.calls().is_empty(), "no guest call for a host log");
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        // nginx's access log holds whatever URL or user agent a visitor sent.
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "limactl",
            &["shell"],
            Output::ok("GET /\u{1b}]0;owned\u{7} HTTP/1.1\tok\n"),
        );
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let mut got = Vec::new();
        logs(
            &h,
            &MemoryStore::default(),
            "t",
            Source::Nginx,
            false,
            5,
            &mut |l| got.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(got, ["GET /\\u{1b}]0;owned\\u{7} HTTP/1.1\tok"]);
    }

    #[test]
    fn sources_parse_by_name() {
        assert_eq!(Source::parse("php"), Some(Source::Php));
        assert_eq!(Source::parse("worker"), Some(Source::Worker));
        assert_eq!(Source::parse("bogus"), None);
    }
}
