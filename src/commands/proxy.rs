//! `bunker proxy serve` (hidden): run by launchd as `com.bunker.proxy`. One listener per project
//! on `127.0.0.1:<ports.proxy>`; logs to `~/.bunker/logs/proxy.log`.
use crate::error::{BunkerError, Result};
use crate::output::{ExitCode, Report};
use crate::paths::{ProjectDir, bunker_home, ensure_private_dir};
use crate::proxy::own::own_addresses;
use crate::proxy::resolve::{SystemConnector, SystemResolver};
use crate::proxy::server::{ProjectListener, serve as serve_listeners};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::net::IpAddr;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Arc, RwLock};

/// `(project, proxy port)` for every project directory with a readable `[ports].proxy`. An invalid
/// config still gets its listener, so the guest reaches a proxy that fails closed.
pub fn discover(home: &Path) -> Vec<(String, u16)> {
    let Ok(entries) = std::fs::read_dir(home) else {
        return vec![];
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            crate::config::validate::validate_project_name(&name).ok()?;
            let text = std::fs::read_to_string(e.path().join("bunker.toml")).ok()?;
            let doc: toml::Table = toml::from_str(&text).ok()?;
            let port = doc.get("ports")?.get("proxy")?.as_integer()?;
            Some((name, u16::try_from(port).ok().filter(|p| *p > 0)?))
        })
        .collect()
}

/// Appends one timestamped line to `~/.bunker/logs/proxy.log` (0600).
fn plog(home: &Path, msg: &str) {
    let dir = home.join("logs");
    if ensure_private_dir(&dir).is_err() {
        return;
    }
    let line = format!(
        "{} {msg}\n",
        crate::time::rfc3339(std::time::SystemTime::now())
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(dir.join("proxy.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

pub struct ServeReport {
    projects: Vec<String>,
}

impl Report for ServeReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "proxy stopped (served: {})", self.projects.join(", "))
    }
    fn json(&self) -> Value {
        json!({"stopped": true, "projects": self.projects})
    }
}

/// Binds every project's proxy port and serves until SIGTERM or SIGINT. New projects need a
/// restart of the agent (`bunker up` does that).
pub fn serve() -> Result<ServeReport> {
    let home = bunker_home();
    let mut listeners = Vec::new();
    for (name, port) in discover(&home) {
        match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => {
                plog(&home, &format!("{name}: listening on 127.0.0.1:{port}"));
                listeners.push(ProjectListener {
                    dir: ProjectDir::under(&home, &name),
                    project: name,
                    listener: l,
                });
            }
            Err(e) => plog(&home, &format!("{name}: cannot bind 127.0.0.1:{port}: {e}")),
        }
    }
    let projects: Vec<String> = listeners.iter().map(|l| l.project.clone()).collect();
    if projects.is_empty() {
        return Err(BunkerError::new(
            "no_projects",
            ExitCode::NotFound,
            "no bunker projects to serve",
        )
        .hint("run bunker init <project> --path <repo>"));
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| BunkerError::io("cannot start the async runtime", e))?;
    let result = rt.block_on(async {
        let own = Arc::new(RwLock::new(own_addresses()));
        let refresher = own.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tick.tick().await;
                *refresher.write().unwrap() = own_addresses();
            }
        });
        let own_fn: Arc<dyn Fn() -> Vec<IpAddr> + Send + Sync> = Arc::new(move || own.read().unwrap().clone());
        let shutdown = async {
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = async { match term.as_mut() { Some(t) => { t.recv().await; } None => std::future::pending::<()>().await } } => {}
            }
        };
        serve_listeners(listeners, Arc::new(SystemResolver), Arc::new(SystemConnector), own_fn, shutdown).await
    });
    plog(&home, "stopped");
    result?;
    if projects.is_empty() {
        return Err(BunkerError::new(
            "no_projects",
            ExitCode::NotFound,
            "no bunker projects to serve",
        )
        .hint("run bunker init <project> --path <repo>"));
    }
    Ok(ServeReport { projects })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discover_reads_proxy_ports_even_from_invalid_configs() {
        let home = tempfile::tempdir().unwrap();
        let mk = |name: &str, body: &str| {
            fs::create_dir_all(home.path().join(name)).unwrap();
            fs::write(home.path().join(name).join("bunker.toml"), body).unwrap();
        };
        mk("good", &crate::config::fixture_config());
        mk(
            "broken",
            "typo = 1\n[ports]\nssh = 60101\nproxy = 18801\nweb = 18901\n",
        );
        mk("noport", "project = 1\n");
        fs::create_dir_all(home.path().join("old")).unwrap();
        fs::write(home.path().join("old/Caddyfile"), "").unwrap();
        fs::create_dir_all(home.path().join("logs")).unwrap();
        let mut got = discover(home.path());
        got.sort();
        assert_eq!(
            got,
            [("broken".to_string(), 18801), ("good".to_string(), 18800)]
        );
    }
}
