//! `~/.bunker/<project>/bunker.toml`: the project's only config file. Unknown keys are errors.
pub mod constraint;
pub mod detect;
pub mod ports;
pub mod validate;

use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use crate::paths::ProjectDir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use validate::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stack {
    Laravel,
}

impl Stack {
    pub fn as_str(self) -> &'static str {
        match self {
            Stack::Laravel => "laravel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub project: PathBuf,
    pub stack: Stack,
    pub php: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub tunnel: Option<String>,
    #[serde(default)]
    pub services: Services,
    #[serde(default)]
    pub workers: BTreeMap<String, String>,
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(default)]
    pub hooks: Hooks,
    #[serde(default)]
    pub vm: Vm,
    pub ports: Ports,
    #[serde(default)]
    pub network: Network,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, SecretMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Services {
    pub mysql: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hooks {
    #[serde(default)]
    pub first_deploy: Vec<String>,
    #[serde(default)]
    pub before_switch: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Vm {
    pub memory: String,
    pub cpus: u32,
    pub disk: String,
}

impl Default for Vm {
    fn default() -> Self {
        Self {
            memory: "2GiB".into(),
            cpus: 2,
            disk: "20GiB".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ports {
    pub ssh: u16,
    pub proxy: u16,
    pub web: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    #[serde(default)]
    pub build: Vec<String>,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    InApp,
    Bound,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretMeta {
    pub kind: SecretKind,
    #[serde(default)]
    pub host: Option<String>,
}

/// MySQL tracks bunker can provision: 8.4 is Percona Server 8.4 LTS (Oracle ships no arm64 noble packages).
pub const SUPPORTED_MYSQL: [&str; 1] = ["8.4"];

fn config_invalid(msg: impl Into<String>) -> BunkerError {
    BunkerError::new("config_invalid", ExitCode::Failure, msg)
}

impl Config {
    /// Parses and validates. Every problem is a `config_invalid` error naming the key.
    pub fn parse(s: &str) -> Result<Config> {
        let c: Config = toml::from_str(s)
            .map_err(|e| config_invalid(format!("bunker.toml: {}", e.message())))?;
        c.validate()
            .map_err(|e| config_invalid(format!("bunker.toml: {}", e.message)))?;
        Ok(c)
    }

    pub fn load(pd: &ProjectDir) -> Result<Config> {
        let path = pd.config();
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                not_found(&pd.name)
            } else {
                BunkerError::io(format!("cannot read {}", path.display()), e)
            }
        })?;
        Config::parse(&text).map_err(|mut e| {
            e.message = e
                .message
                .replacen("bunker.toml", &path.display().to_string(), 1);
            e
        })
    }

    fn validate(&self) -> Result<()> {
        let key =
            |k: &str, r: Result<()>| r.map_err(|e| config_invalid(format!("{k}: {}", e.message)));
        key(
            "project",
            validate_path("project", &self.project.to_string_lossy()),
        )?;
        key("php", validate_php(&self.php))?;
        for e in &self.extensions {
            key("extensions", validate_extension(e))?;
        }
        if let Some(n) = &self.node
            && !(n.len() <= 3 && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(config_invalid(format!(
                "node: {n:?} must be a major version like \"22\""
            )));
        }
        if let Some(d) = &self.domain {
            key("domain", validate_domain(d))?;
        }
        if let Some(t) = &self.tunnel {
            key("tunnel", validate_tunnel_name(t))?;
        }
        if self.tunnel.is_some() != self.domain.is_some() {
            return Err(config_invalid(
                "domain and tunnel go together: set both or neither",
            ));
        }
        if let Some(m) = &self.services.mysql
            && !SUPPORTED_MYSQL.contains(&m.as_str())
        {
            return Err(config_invalid(format!(
                "services.mysql: {m:?} is not supported; supported: {}",
                SUPPORTED_MYSQL.join(", ")
            )));
        }
        for (name, cmd) in &self.workers {
            key("workers", validate_project_name(name))?;
            single_line(&format!("workers.{name}"), cmd)?;
        }
        for (list, cmds) in [
            ("hooks.first_deploy", &self.hooks.first_deploy),
            ("hooks.before_switch", &self.hooks.before_switch),
        ] {
            for c in cmds {
                single_line(list, c)?;
            }
        }
        key("vm.memory", validate_size(&self.vm.memory))?;
        key("vm.disk", validate_size(&self.vm.disk))?;
        if !(1..=16).contains(&self.vm.cpus) {
            return Err(config_invalid("vm.cpus must be 1-16"));
        }
        for (k, p, range) in [
            ("ports.ssh", self.ports.ssh, ports::SSH_RANGE),
            ("ports.proxy", self.ports.proxy, ports::PROXY_RANGE),
            ("ports.web", self.ports.web, ports::WEB_RANGE),
        ] {
            if !range.contains(&p) {
                return Err(config_invalid(format!(
                    "{k}: {p} is outside {}-{}",
                    range.start(),
                    range.end()
                )));
            }
        }
        for (list, entries) in [
            ("network.build", &self.network.build),
            ("network.allow", &self.network.allow),
            ("network.deny", &self.network.deny),
        ] {
            for h in entries {
                key(list, validate_host_pattern(h))?;
            }
        }
        for (k, v) in &self.env {
            key("env", validate_env_key(k))?;
            if v.contains(['\n', '\r', '\0']) {
                return Err(config_invalid(format!(
                    "env.{k}: plain settings are single-line"
                )));
            }
            if self.secrets.contains_key(k) {
                return Err(config_invalid(format!(
                    "{k} is in both [env] and [secrets]"
                )));
            }
        }
        for (k, meta) in &self.secrets {
            key("secrets", validate_env_key(k))?;
            match (meta.kind, &meta.host) {
                (SecretKind::Bound, Some(h)) => {
                    key(&format!("secrets.{k}.host"), validate_host_pattern(h))?
                }
                (SecretKind::Bound, None) => {
                    return Err(config_invalid(format!(
                        "secrets.{k}: a bound secret needs host"
                    )));
                }
                (SecretKind::InApp, Some(_)) => {
                    return Err(config_invalid(format!(
                        "secrets.{k}: an in_app secret takes no host"
                    )));
                }
                (SecretKind::InApp, None) => {}
            }
        }
        Ok(())
    }
}

fn single_line(what: &str, cmd: &str) -> Result<()> {
    if cmd.trim().is_empty() || cmd.contains(['\n', '\r', '\0']) {
        return Err(config_invalid(format!(
            "{what}: a command must be one non-empty line"
        )));
    }
    Ok(())
}

fn not_found(name: &str) -> BunkerError {
    BunkerError::new(
        "project_not_found",
        ExitCode::NotFound,
        format!("no bunker project named {name:?}"),
    )
    .hint("run bunker init <project> --path <repo>")
}

/// Old v0.1 directories already reported by this process, so each notice prints once.
static NOTICED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// True for a v0.1 project directory (a Caddyfile and no bunker.toml); prints the notice once.
fn skip_old_layout(dir: &Path, notices: &mut dyn Write) -> bool {
    if dir.join("bunker.toml").exists() || !dir.join("Caddyfile").exists() {
        return false;
    }
    if NOTICED.lock().unwrap().insert(dir.to_path_buf()) {
        let name = dir.file_name().unwrap_or_default().to_string_lossy();
        let _ = writeln!(
            notices,
            "~/.bunker/{name} is a bunker v0.1 project; v2 ignores it"
        );
    }
    true
}

/// Only the `project` key, so one broken bunker.toml doesn't stop resolving the others.
fn project_path_of(config: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(config).ok()?;
    let doc: toml::Table = toml::from_str(&text).ok()?;
    doc.get("project")?.as_str().map(PathBuf::from)
}

/// Finds the project: by name under `bunker_home()`, or the one whose `project` path holds `cwd`.
pub fn resolve_project(arg: Option<&str>, cwd: &Path) -> Result<ProjectDir> {
    resolve_project_in(
        &crate::paths::bunker_home(),
        arg,
        cwd,
        &mut std::io::stderr(),
    )
}

pub fn resolve_project_in(
    home: &Path,
    arg: Option<&str>,
    cwd: &Path,
    notices: &mut dyn Write,
) -> Result<ProjectDir> {
    if let Some(name) = arg {
        validate_project_name(name)?;
        let pd = ProjectDir::under(home, name);
        if skip_old_layout(&pd.root, notices) || !pd.config().is_file() {
            return Err(not_found(name));
        }
        return Ok(pd);
    }
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut best: Option<(usize, ProjectDir)> = None;
    let entries = std::fs::read_dir(home).into_iter().flatten().flatten();
    for entry in entries {
        let dir = entry.path();
        if !dir.is_dir() || skip_old_layout(&dir, notices) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(project) = project_path_of(&dir.join("bunker.toml")) else {
            continue;
        };
        let project = project.canonicalize().unwrap_or(project);
        let depth = project.components().count();
        if cwd.starts_with(&project) && best.as_ref().is_none_or(|(d, _)| depth > *d) {
            best = Some((depth, ProjectDir::under(home, &name)));
        }
    }
    best.map(|(_, pd)| pd).ok_or_else(|| {
        BunkerError::new(
            "project_not_found",
            ExitCode::NotFound,
            format!("{} is not inside any bunker project", cwd.display()),
        )
        .hint("name the project, or run bunker init <project> --path <repo>")
    })
}

/// The spec's example `bunker.toml` plus `[ports]`; tests that need a valid config start here.
#[cfg(test)]
pub(crate) fn fixture_config() -> String {
    r#"project    = "/Users/inggo/Code/life-os"   # source repo on the mini
stack      = "laravel"
php        = "8.4"
extensions = ["intl", "bcmath"]
domain     = "life-os.bangkila.com"
tunnel     = "life-os"

[services]
mysql = "8.4"

[workers]
queue = "php artisan queue:work --tries=3 --timeout=30"

[schedule]
enabled = false          # Laravel scheduler; Life OS has no scheduled tasks yet

[hooks]                  # run inside the VM, in the new release, as the app user
first_deploy  = ["php artisan passport:keys"]
before_switch = ["php artisan migrate --force"]

[vm]
memory = "2GiB"
cpus   = 2
disk   = "20GiB"

[ports]
ssh   = 60100
proxy = 18800
web   = 18900

[network]
build = []   # extra hosts allowed during a deploy, on top of bunker's built-in list
allow = []   # hosts the running site may reach; edited by `bunker net allow`
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn unknown_key_is_an_error() {
        let s = fixture_config().replace("memory = \"2GiB\"", "memry = \"2GiB\"");
        let e = Config::parse(&s).unwrap_err();
        assert_eq!(e.code, "config_invalid");
        assert!(e.message.contains("memry"), "{}", e.message);
    }

    #[test]
    fn spec_example_config_parses() {
        let c = Config::parse(&fixture_config()).unwrap();
        assert_eq!(c.php, "8.4");
        assert_eq!(
            c.workers["queue"],
            "php artisan queue:work --tries=3 --timeout=30"
        );
        assert!(c.network.allow.is_empty());
        assert_eq!(
            c.ports,
            Ports {
                ssh: 60100,
                proxy: 18800,
                web: 18900
            }
        );
        assert_eq!(c.services.mysql.as_deref(), Some("8.4"));
    }

    #[test]
    fn invalid_values_are_config_errors() {
        for (from, to) in [
            ("php        = \"8.4\"", "php = \"8.9\""),
            ("allow = []", "allow = [\"10.0.0.1\"]"),
            ("ssh   = 60100", "ssh = 22"),
            ("mysql = \"8.4\"", "mysql = \"8.0\""),
            ("memory = \"2GiB\"", "memory = \"2G\""),
            (
                "[ports]",
                "[secrets]\nAPP_KEY = { kind = \"bound\" }\n\n[ports]",
            ),
            ("[ports]", "[env]\nlower = \"x\"\n\n[ports]"),
        ] {
            let s = fixture_config().replace(from, to);
            assert_ne!(s, fixture_config(), "fixture lost {from}");
            let e = Config::parse(&s).expect_err(to);
            assert_eq!(e.code, "config_invalid", "{to}");
        }
    }

    fn project_at(home: &Path, name: &str, repo: &Path) {
        let pd = ProjectDir::under(home, name);
        fs::create_dir_all(&pd.root).unwrap();
        let s = fixture_config().replace("/Users/inggo/Code/life-os", repo.to_str().unwrap());
        fs::write(pd.config(), s).unwrap();
    }

    #[test]
    fn resolve_project_from_nested_cwd() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        fs::create_dir_all(repo.path().join("app/Http")).unwrap();
        project_at(home.path(), "life-os", repo.path());
        project_at(home.path(), "other", other.path());
        let mut notes = Vec::new();
        let pd = resolve_project_in(home.path(), None, &repo.path().join("app/Http"), &mut notes)
            .unwrap();
        assert_eq!(pd.name, "life-os");
        let pd = resolve_project_in(home.path(), Some("other"), repo.path(), &mut notes).unwrap();
        assert_eq!(pd.name, "other");
        let e = resolve_project_in(home.path(), Some("nope"), repo.path(), &mut notes).unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("project_not_found", crate::output::ExitCode::NotFound)
        );
        let e = resolve_project_in(home.path(), None, Path::new("/"), &mut notes).unwrap_err();
        assert_eq!(e.code, "project_not_found");
        assert!(notes.is_empty());
    }

    #[test]
    fn old_layout_is_skipped_with_notice() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("oldsite")).unwrap();
        fs::write(home.path().join("oldsite/Caddyfile"), ":80\n").unwrap();
        let mut notes = Vec::new();
        let e = resolve_project_in(home.path(), None, home.path(), &mut notes).unwrap_err();
        assert_eq!(e.code, "project_not_found");
        let e =
            resolve_project_in(home.path(), Some("oldsite"), home.path(), &mut notes).unwrap_err();
        assert_eq!(e.code, "project_not_found");
        let notes = String::from_utf8(notes).unwrap();
        let line = "~/.bunker/oldsite is a bunker v0.1 project; v2 ignores it\n";
        assert_eq!(notes.matches(line).count(), 1, "{notes}");
    }
}
