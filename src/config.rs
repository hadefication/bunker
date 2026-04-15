use std::collections::HashMap;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::framework::FrameworkKind;

/// Validate a project name: only lowercase alphanumeric and hyphens
fn validate_project_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("Project name cannot be empty");
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        anyhow::bail!(
            "Invalid project name '{}': only lowercase letters, digits, and hyphens allowed",
            name
        );
    }
    Ok(())
}

/// Validate a file path: must be absolute, no newlines or null bytes
fn validate_path(label: &str, path: &str) -> anyhow::Result<()> {
    if path.contains('\0') || path.contains('\n') || path.contains('\r') {
        anyhow::bail!("{} contains invalid characters (null bytes or newlines)", label);
    }
    if !Path::new(path).is_absolute() {
        anyhow::bail!("{} must be an absolute path, got: {}", label, path);
    }
    Ok(())
}

pub fn bunker_home() -> PathBuf {
    dirs().0
}

pub fn launch_agents_dir() -> PathBuf {
    dirs().1
}

fn dirs() -> (PathBuf, PathBuf) {
    let home = env::var("HOME").expect("HOME not set");
    (
        PathBuf::from(&home).join(".bunker"),
        PathBuf::from(&home).join("Library/LaunchAgents"),
    )
}

#[derive(Debug, Clone)]
pub struct ProjectConfig {
    pub project_name: String,
    pub project_path: String,
    pub port: u16,
    pub domain: String,
    pub tunnel_name: String,
    pub tunnel_uuid: String,
    pub php_path: String,
    pub frankenphp_path: String,
    pub cloudflared_path: String,
    pub scheduler_enabled: bool,
    pub framework: FrameworkKind,
}

impl ProjectConfig {
    pub fn project_dir(&self) -> PathBuf {
        bunker_home().join(&self.project_name)
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.project_dir().join("logs")
    }

    pub fn conf_path(&self) -> PathBuf {
        self.project_dir().join("bunker.conf")
    }

    pub fn service_labels(&self) -> Vec<String> {
        let mut labels = vec![
            format!("com.{}.server", self.project_name),
            format!("com.{}.tunnel", self.project_name),
            format!("com.{}.queue", self.project_name),
        ];
        if self.scheduler_enabled {
            labels.push(format!("com.{}.scheduler", self.project_name));
        }
        labels
    }

    pub fn write(&self) -> anyhow::Result<()> {
        let dir = self.project_dir();
        fs::create_dir_all(dir.join("logs"))?;

        let content = format!(
            r#"PROJECT_NAME="{}"
PROJECT_PATH="{}"
PORT={}
DOMAIN="{}"
TUNNEL_NAME="{}"
TUNNEL_UUID="{}"
PHP_PATH="{}"
FRANKENPHP_PATH="{}"
CLOUDFLARED_PATH="{}"
SCHEDULER_ENABLED="{}"
FRAMEWORK="{}"
"#,
            self.project_name,
            self.project_path,
            self.port,
            self.domain,
            self.tunnel_name,
            self.tunnel_uuid,
            self.php_path,
            self.frankenphp_path,
            self.cloudflared_path,
            self.scheduler_enabled,
            self.framework.as_str(),
        );

        let conf = self.conf_path();
        fs::write(&conf, content)?;
        fs::set_permissions(&conf, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    pub fn load(project: &str) -> anyhow::Result<Self> {
        let conf_path = bunker_home().join(project).join("bunker.conf");
        if !conf_path.exists() {
            anyhow::bail!("Config not found: {}", conf_path.display());
        }

        let content = fs::read_to_string(&conf_path)?;
        let map = parse_conf(&content);

        let get = |key: &str| -> anyhow::Result<String> {
            map.get(key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Missing key '{}' in {}", key, conf_path.display()))
        };

        let project_name = get("PROJECT_NAME")?;
        let project_path = get("PROJECT_PATH")?;
        let php_path = get("PHP_PATH")?;
        let frankenphp_path = get("FRANKENPHP_PATH")?;
        let cloudflared_path = get("CLOUDFLARED_PATH")?;

        validate_project_name(&project_name)?;
        validate_path("PROJECT_PATH", &project_path)?;
        validate_path("PHP_PATH", &php_path)?;
        validate_path("FRANKENPHP_PATH", &frankenphp_path)?;
        validate_path("CLOUDFLARED_PATH", &cloudflared_path)?;

        Ok(Self {
            project_name,
            project_path,
            port: get("PORT")?.parse()?,
            domain: get("DOMAIN")?,
            tunnel_name: get("TUNNEL_NAME")?,
            tunnel_uuid: get("TUNNEL_UUID")?,
            php_path,
            frankenphp_path,
            cloudflared_path,
            scheduler_enabled: get("SCHEDULER_ENABLED").unwrap_or_default() == "true",
            framework: map
                .get("FRAMEWORK")
                .map(|s| FrameworkKind::from_str(s))
                .unwrap_or(FrameworkKind::Laravel),
        })
    }
}

fn parse_conf(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, val)) = line.split_once('=') {
            let val = val.trim_matches('"');
            map.insert(key.to_string(), val.to_string());
        }
    }
    map
}

/// Resolve project name from optional arg or CWD
pub fn resolve_project(project: Option<String>) -> anyhow::Result<String> {
    if let Some(name) = project {
        validate_project_name(&name)?;
        let conf = bunker_home().join(&name).join("bunker.conf");
        if conf.exists() {
            return Ok(name);
        }
        anyhow::bail!("Project '{}' not found. Run 'bunker init' first.", name);
    }

    // Try CWD
    let cwd = env::current_dir()?;
    let cwd_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();

    let conf = bunker_home().join(&cwd_name).join("bunker.conf");
    if conf.exists() {
        return Ok(cwd_name);
    }

    anyhow::bail!("Not a bunkered project. Run 'bunker init' first.");
}

/// Check if a port is available
pub fn port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Find an unused port starting from base
pub fn suggest_port(base: u16) -> u16 {
    let mut port = base;
    while !port_available(port) {
        port += 1;
    }
    port
}

/// Convert string to kebab-case
pub fn to_kebab(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
