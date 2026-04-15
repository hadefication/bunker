use std::collections::HashMap;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::framework::FrameworkKind;

/// Validate a project name: only lowercase alphanumeric and hyphens
pub fn validate_project_name(name: &str) -> anyhow::Result<()> {
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

/// Validate a tunnel name: alphanumeric and hyphens, no leading dash
pub fn validate_tunnel_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("Tunnel name cannot be empty");
    }
    if name.starts_with('-') {
        anyhow::bail!("Tunnel name '{}' must not start with a hyphen", name);
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        anyhow::bail!(
            "Invalid tunnel name '{}': only letters, digits, and hyphens allowed",
            name
        );
    }
    Ok(())
}

/// Validate a domain: must contain a dot, no leading dash, reasonable DNS charset
pub fn validate_domain(domain: &str) -> anyhow::Result<()> {
    if domain.is_empty() {
        anyhow::bail!("Domain cannot be empty");
    }
    if domain.starts_with('-') {
        anyhow::bail!("Domain '{}' must not start with a hyphen", domain);
    }
    if !domain.contains('.') {
        anyhow::bail!("Domain '{}' must contain at least one dot", domain);
    }
    if !domain.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') {
        anyhow::bail!(
            "Invalid domain '{}': only letters, digits, hyphens, and dots allowed",
            domain
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
            // Strip surrounding quotes, then strip inline comments outside quotes
            let val = val.trim();
            let val = if let Some(inner) = val.strip_prefix('"') {
                // Quoted value: take content up to closing quote
                inner.split_once('"').map(|(s, _)| s).unwrap_or(inner)
            } else {
                // Unquoted value: strip inline comments
                val.split('#').next().unwrap_or(val).trim()
            };
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

    if validate_project_name(&cwd_name).is_ok() {
        let conf = bunker_home().join(&cwd_name).join("bunker.conf");
        if conf.exists() {
            return Ok(cwd_name);
        }
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
        if port == u16::MAX {
            return base; // give up, return the base and let the user decide
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- to_kebab ---

    #[test]
    fn kebab_simple() {
        assert_eq!(to_kebab("MyProject"), "myproject");
    }

    #[test]
    fn kebab_spaces_and_special() {
        assert_eq!(to_kebab("My Cool Project!"), "my-cool-project");
    }

    #[test]
    fn kebab_already_kebab() {
        assert_eq!(to_kebab("my-project"), "my-project");
    }

    #[test]
    fn kebab_consecutive_special() {
        assert_eq!(to_kebab("my---project"), "my-project");
    }

    #[test]
    fn kebab_leading_trailing() {
        assert_eq!(to_kebab("--my-project--"), "my-project");
    }

    // --- validate_project_name ---

    #[test]
    fn valid_project_names() {
        assert!(validate_project_name("my-app").is_ok());
        assert!(validate_project_name("app123").is_ok());
        assert!(validate_project_name("a").is_ok());
    }

    #[test]
    fn invalid_project_names() {
        assert!(validate_project_name("").is_err());
        assert!(validate_project_name("My-App").is_err());
        assert!(validate_project_name("../evil").is_err());
        assert!(validate_project_name("my app").is_err());
        assert!(validate_project_name("my_app").is_err());
    }

    // --- validate_tunnel_name ---

    #[test]
    fn valid_tunnel_names() {
        assert!(validate_tunnel_name("my-tunnel").is_ok());
        assert!(validate_tunnel_name("Tunnel123").is_ok());
    }

    #[test]
    fn invalid_tunnel_names() {
        assert!(validate_tunnel_name("").is_err());
        assert!(validate_tunnel_name("-leading").is_err());
        assert!(validate_tunnel_name("has spaces").is_err());
        assert!(validate_tunnel_name("--config").is_err());
    }

    // --- validate_domain ---

    #[test]
    fn valid_domains() {
        assert!(validate_domain("example.com").is_ok());
        assert!(validate_domain("my-app.example.com").is_ok());
        assert!(validate_domain("a.b.c.d.com").is_ok());
    }

    #[test]
    fn invalid_domains() {
        assert!(validate_domain("").is_err());
        assert!(validate_domain("nodot").is_err());
        assert!(validate_domain("-leading.com").is_err());
        assert!(validate_domain("has space.com").is_err());
    }

    // --- validate_path ---

    #[test]
    fn valid_paths() {
        assert!(validate_path("test", "/usr/bin/php").is_ok());
        assert!(validate_path("test", "/opt/homebrew/bin/frankenphp").is_ok());
    }

    #[test]
    fn invalid_paths() {
        assert!(validate_path("test", "relative/path").is_err());
        assert!(validate_path("test", "/path/with\nnewline").is_err());
        assert!(validate_path("test", "/path/with\0null").is_err());
    }

    // --- parse_conf ---

    #[test]
    fn parse_conf_basic() {
        let content = r#"
PROJECT_NAME="my-app"
PORT=8700
"#;
        let map = parse_conf(content);
        assert_eq!(map.get("PROJECT_NAME").unwrap(), "my-app");
        assert_eq!(map.get("PORT").unwrap(), "8700");
    }

    #[test]
    fn parse_conf_inline_comment() {
        let content = "PORT=8700 # default port\n";
        let map = parse_conf(content);
        assert_eq!(map.get("PORT").unwrap(), "8700");
    }

    #[test]
    fn parse_conf_quoted_with_hash() {
        let content = r#"DOMAIN="my-app.example.com""#;
        let map = parse_conf(content);
        assert_eq!(map.get("DOMAIN").unwrap(), "my-app.example.com");
    }

    #[test]
    fn parse_conf_comment_lines() {
        let content = "# this is a comment\nPORT=8700\n";
        let map = parse_conf(content);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("PORT").unwrap(), "8700");
    }

    #[test]
    fn parse_conf_path_with_spaces() {
        let content = r#"PHP_PATH="/Users/me/Library/Application Support/Herd/bin/php""#;
        let map = parse_conf(content);
        assert_eq!(
            map.get("PHP_PATH").unwrap(),
            "/Users/me/Library/Application Support/Herd/bin/php"
        );
    }

    // --- suggest_port ---

    #[test]
    fn suggest_port_returns_port() {
        let port = suggest_port(49152); // high port, likely available
        assert!(port >= 49152);
    }
}
