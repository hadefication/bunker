//! `bunker init <project> --path <repo>`: detect the app and write a commented `bunker.toml`.
use crate::config::detect::{Detected, detect};
use crate::config::ports::{allocate_ports, port_is_free};
use crate::config::validate::*;
use crate::config::{Config, Ports};
use crate::error::{BunkerError, Result};
use crate::output::{ExitCode, Report};
use crate::paths::{ProjectDir, bunker_home, ensure_private_dir, write_private};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table, value};

pub struct InitArgs {
    pub project: String,
    pub path: PathBuf,
    pub domain: Option<String>,
    pub tunnel: Option<String>,
    pub php: Option<String>,
    pub force: bool,
}

pub struct InitReport {
    project: String,
    config: PathBuf,
    repo: PathBuf,
    detected: Detected,
    php: String,
    domain: Option<String>,
    tunnel: Option<String>,
    ports: Ports,
}

impl Report for InitReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        writeln!(w, "wrote {}", self.config.display())?;
        writeln!(w, "  repo      {}", self.repo.display())?;
        writeln!(
            w,
            "  stack     {} (PHP {})",
            self.detected.stack.as_str(),
            self.php
        )?;
        if !self.detected.extensions.is_empty() {
            writeln!(w, "  ext       {}", self.detected.extensions.join(", "))?;
        }
        if let Some(n) = &self.detected.node {
            writeln!(w, "  node      {n} (package.json has a build script)")?;
        }
        if self.detected.passport {
            writeln!(
                w,
                "  passport  yes (keys go in the Keychain, loaded from .env)"
            )?;
        }
        match (&self.domain, &self.tunnel) {
            (Some(d), Some(t)) => writeln!(w, "  domain    {d} via tunnel {t}")?,
            _ => writeln!(
                w,
                "  domain    none (no tunnel; the site is only reachable locally)"
            )?,
        }
        writeln!(
            w,
            "  ports     ssh {}, proxy {}, web {}",
            self.ports.ssh, self.ports.proxy, self.ports.web
        )?;
        writeln!(w, "next: review the file, then bunker up {}", self.project)
    }

    fn json(&self) -> Value {
        json!({
            "project": self.project,
            "config": self.config,
            "repo": self.repo,
            "stack": self.detected.stack.as_str(),
            "php": self.php,
            "extensions": self.detected.extensions,
            "node": self.detected.node,
            "passport": self.detected.passport,
            "domain": self.domain,
            "tunnel": self.tunnel,
            "ports": self.ports,
        })
    }
}

pub fn run(a: InitArgs) -> Result<InitReport> {
    run_in(&bunker_home(), a, &port_is_free)
}

pub fn run_in(home: &Path, a: InitArgs, is_free: &dyn Fn(u16) -> bool) -> Result<InitReport> {
    validate_project_name(&a.project)?;
    if let Some(d) = &a.domain {
        validate_domain(d)?;
    }
    let tunnel = match (&a.tunnel, &a.domain) {
        (Some(t), Some(_)) => Some(t.clone()),
        (None, Some(_)) => Some(a.project.clone()),
        (Some(_), None) => {
            return Err(BunkerError::new(
                "invalid_value",
                ExitCode::Usage,
                "--tunnel needs --domain",
            ));
        }
        (None, None) => None,
    };
    if let Some(t) = &tunnel {
        validate_tunnel_name(t)?;
    }
    if let Some(v) = &a.php {
        validate_php(v)?;
    }
    let repo = a.path.canonicalize().map_err(|e| {
        BunkerError::new(
            "repo_not_found",
            ExitCode::NotFound,
            format!("cannot open {}: {e}", a.path.display()),
        )
    })?;
    validate_path("--path", &repo.to_string_lossy())?;
    let detected = detect(&repo)?;
    let php = match (&a.php, &detected.php, &detected.php_constraint) {
        (Some(v), _, _) => v.clone(),
        (None, Some(v), _) => v.clone(),
        // detect() already found this unreadable; rerun it for the error and its hint.
        (None, None, Some(c)) => crate::config::constraint::php_from_constraint(c)?,
        (None, None, None) => {
            return Err(BunkerError::new(
                "php_constraint",
                ExitCode::Refused,
                "composer.json has no php requirement",
            )
            .hint("pass --php <version>"));
        }
    };

    let pd = ProjectDir::under(home, &a.project);
    if pd.config().exists() && !a.force {
        return Err(BunkerError::new(
            "config_exists",
            ExitCode::Refused,
            format!("{} already exists", pd.config().display()),
        )
        .hint("pass --force to overwrite it"));
    }
    let taken = other_projects_ports(home, &a.project);
    let ports = allocate_ports(&taken, is_free)?;

    let report = InitReport {
        project: a.project.clone(),
        config: pd.config(),
        repo,
        detected,
        php,
        domain: a.domain.clone(),
        tunnel,
        ports,
    };
    let text = render(&report);
    Config::parse(&text).map_err(|e| {
        BunkerError::new(
            "internal",
            ExitCode::Failure,
            format!("init built an invalid config: {}", e.message),
        )
    })?;
    ensure_private_dir(home)?;
    ensure_private_dir(&pd.root)?;
    write_private(&pd.config(), text.as_bytes())?;
    Ok(report)
}

/// Ports held by every other project's bunker.toml, valid or not.
fn other_projects_ports(home: &Path, me: &str) -> Vec<Ports> {
    let Ok(entries) = std::fs::read_dir(home) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.file_name() != me)
        .filter_map(|e| std::fs::read_to_string(e.path().join("bunker.toml")).ok())
        .filter_map(|text| {
            let doc: toml::Table = toml::from_str(&text).ok()?;
            doc.get("ports")?.clone().try_into().ok()
        })
        .collect()
}

fn strings(items: &[String]) -> Item {
    value(items.iter().collect::<Array>())
}

fn commented(key_comment: &str) -> String {
    format!("\n# {key_comment}\n")
}

/// The commented bunker.toml for a fresh project.
fn render(r: &InitReport) -> String {
    let mut doc = DocumentMut::new();
    doc.decor_mut().set_prefix(format!(
        "# bunker config for {}. Edit by hand or with bunker net/env; unknown keys are errors.\n",
        r.project
    ));
    doc["project"] = value(r.repo.to_string_lossy().as_ref());
    doc["stack"] = value(r.detected.stack.as_str());
    doc["php"] = value(r.php.as_str());
    doc["extensions"] = strings(&r.detected.extensions);
    if let Some(n) = &r.detected.node {
        doc["node"] = value(n.as_str());
    }
    if let (Some(d), Some(t)) = (&r.domain, &r.tunnel) {
        doc["domain"] = value(d.as_str());
        doc["tunnel"] = value(t.as_str());
    }

    let mut t = Table::new();
    t.decor_mut()
        .set_prefix(commented("Services installed inside the VM."));
    t["mysql"] = value("8.4");
    doc["services"] = Item::Table(t);

    let mut t = Table::new();
    t.decor_mut().set_prefix(commented(
        "Long-running workers, one systemd unit each, run as the app user.",
    ));
    t["queue"] = value("php artisan queue:work --tries=3 --timeout=30");
    doc["workers"] = Item::Table(t);

    let mut t = Table::new();
    t.decor_mut().set_prefix(commented(
        "The Laravel scheduler (schedule:run every minute).",
    ));
    t["enabled"] = value(false);
    doc["schedule"] = Item::Table(t);

    let mut t = Table::new();
    t.decor_mut().set_prefix(commented(
        "Run inside the VM, in the new release, as the app user.",
    ));
    t["first_deploy"] = strings(&[]);
    t["before_switch"] = strings(&["php artisan migrate --force".to_string()]);
    doc["hooks"] = Item::Table(t);

    let mut t = Table::new();
    t.decor_mut().set_prefix(commented(
        "The VM's size. Re-run bunker up after changing it.",
    ));
    t["memory"] = value("2GiB");
    t["cpus"] = value(2);
    t["disk"] = value("20GiB");
    doc["vm"] = Item::Table(t);

    let mut t = Table::new();
    t.decor_mut().set_prefix(commented(
        "Host ports, picked by init so projects never collide.",
    ));
    t["ssh"] = value(i64::from(r.ports.ssh));
    t["proxy"] = value(i64::from(r.ports.proxy));
    t["web"] = value(i64::from(r.ports.web));
    doc["ports"] = Item::Table(t);

    let mut t = Table::new();
    t.decor_mut().set_prefix(commented(
        "Outbound allowlist. build: extra hosts during a deploy, on top of the built-in list.\n# allow: hosts the running site may reach. deny wins over both. Edit with bunker net allow|deny|rm.",
    ));
    t["build"] = strings(&[]);
    t["allow"] = strings(&[]);
    t["deny"] = strings(&[]);
    doc["network"] = Item::Table(t);
    doc.to_string()
}
