//! Input validation. Every value that reaches a file, a command line or the guest passes here.
use crate::error::{BunkerError, Result};

pub const SUPPORTED_PHP: [&str; 11] = [
    "7.0", "7.1", "7.2", "7.3", "7.4", "8.0", "8.1", "8.2", "8.3", "8.4", "8.5",
];

fn invalid(msg: impl Into<String>) -> BunkerError {
    BunkerError::new("invalid_value", crate::output::ExitCode::Usage, msg)
}

/// Lowercase letters, digits and hyphens, no leading hyphen, at most 64 characters.
pub fn validate_project_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 || name.starts_with('-') {
        return Err(invalid(format!(
            "invalid project name {name:?}: 1-64 characters, not starting with a hyphen"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(invalid(format!(
            "invalid project name {name:?}: only lowercase letters, digits and hyphens"
        )));
    }
    Ok(())
}

/// Letters, digits and hyphens, no leading hyphen, at most 64 characters.
pub fn validate_tunnel_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('-')
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(invalid(format!(
            "invalid tunnel name {name:?}: 1-64 letters, digits and hyphens, not starting with a hyphen"
        )));
    }
    Ok(())
}

/// A DNS name with at least one dot: letters, digits, hyphens; labels of 1-63 characters.
pub fn validate_domain(domain: &str) -> Result<()> {
    if !is_dns_name(domain) || !domain.contains('.') {
        return Err(invalid(format!(
            "invalid domain {domain:?}: a hostname like app.example.com"
        )));
    }
    Ok(())
}

/// A DNS name: labels of 1-63 letters, digits and inner hyphens, at most 253 characters.
pub fn is_dns_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Absolute, no `..`, no control characters, no shell metacharacters.
pub fn validate_path(label: &str, path: &str) -> Result<()> {
    let p = std::path::Path::new(path);
    if path.chars().any(char::is_control) {
        return Err(invalid(format!("{label} contains control characters")));
    }
    if !p.is_absolute() {
        return Err(invalid(format!(
            "{label} must be an absolute path, got {path:?}"
        )));
    }
    if p.components().any(|c| c == std::path::Component::ParentDir) {
        return Err(invalid(format!(
            "{label} must not contain '..', got {path:?}"
        )));
    }
    if path.contains([
        '"', '\'', '`', '$', '\\', '(', ')', ';', '&', '|', '>', '<', '*', '?',
    ]) {
        return Err(invalid(format!(
            "{label} contains shell metacharacters, got {path:?}"
        )));
    }
    Ok(())
}

/// An allowlist entry: `host`, `*.domain` (subdomains only), or a public IP literal, each with an
/// optional `:port`. Hosts compare case-insensitively and a trailing dot is ignored.
pub fn validate_host_pattern(p: &str) -> Result<()> {
    let bad = |why: &str| {
        invalid(format!("invalid host pattern {p:?}: {why}"))
            .hint("use host, *.domain or a public IP, optionally with :port")
    };
    let (host, port) = match p.rsplit_once(':') {
        Some((h, port)) if !h.contains(':') => (h, Some(port)),
        Some(_) => return Err(bad("IPv6 literals are not supported")),
        None => (p, None),
    };
    if let Some(port) = port {
        match port.parse::<u16>() {
            Ok(n) if n > 0 && port.chars().all(|c| c.is_ascii_digit()) => {}
            _ => return Err(bad("the port must be 1-65535")),
        }
    }
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if crate::proxy::addr::is_internal(ip, &[]) {
            return Err(BunkerError::new(
                "host_internal",
                crate::output::ExitCode::Refused,
                format!("{p:?} is an internal address; the proxy never lets a VM reach one"),
            ));
        }
        return Ok(());
    }
    let (name, wildcard) = match host.strip_prefix("*.") {
        Some(rest) => (rest, true),
        None => (host.as_str(), false),
    };
    if !is_dns_name(name) {
        return Err(bad("not a hostname"));
    }
    if wildcard && name.split('.').count() < 2 {
        return Err(bad(
            "*. needs at least two labels after it, like *.example.com",
        ));
    }
    if !wildcard && name.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err(bad("not a valid IP address"));
    }
    Ok(())
}

pub fn validate_php(v: &str) -> Result<()> {
    if SUPPORTED_PHP.contains(&v) {
        Ok(())
    } else {
        Err(invalid(format!(
            "unsupported PHP version {v:?}; supported: {}",
            SUPPORTED_PHP.join(", ")
        )))
    }
}

/// A PHP extension name as apt and `php -m` spell it: `^[a-z0-9_]+$`.
pub fn validate_extension(e: &str) -> Result<()> {
    if !e.is_empty()
        && e.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        Ok(())
    } else {
        Err(invalid(format!(
            "invalid PHP extension {e:?}: lowercase letters, digits and _"
        )))
    }
}

/// `^\d+(MiB|GiB)$`.
pub fn validate_size(s: &str) -> Result<()> {
    let digits = s.strip_suffix("MiB").or_else(|| s.strip_suffix("GiB"));
    match digits {
        Some(d) if !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()) => Ok(()),
        _ => Err(invalid(format!(
            "invalid size {s:?}: a number followed by MiB or GiB"
        ))),
    }
}

/// `^[A-Z][A-Z0-9_]*$`.
pub fn validate_env_key(k: &str) -> Result<()> {
    let mut chars = k.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "invalid env key {k:?}: uppercase letters, digits and _, starting with a letter"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_pattern_validation() {
        for ok in [
            "example.com",
            "*.github.com",
            "api.example.com:8443",
            "93.184.215.14",
            "Example.COM.",
        ] {
            assert!(validate_host_pattern(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in [
            "*",
            "*.com",
            "exa mple.com",
            "http://x.com",
            "10.0.0.1",
            "example.com:0",
            "",
            "a.*.example.com",
            "example.com:99999",
            "127.0.0.1:443",
            "[::1]",
            "-x.example.com",
        ] {
            let e = validate_host_pattern(bad).expect_err(bad);
            let want = if bad.starts_with("10.") || bad.starts_with("127.") {
                "host_internal"
            } else {
                "invalid_value"
            };
            assert_eq!(e.code, want, "{bad}");
        }
    }

    #[test]
    fn scalar_validators() {
        assert!(validate_php("8.4").is_ok());
        assert!(validate_php("8.6").is_err());
        assert!(validate_extension("pdo_mysql").is_ok());
        assert!(validate_extension("Intl").is_err());
        assert!(validate_size("2GiB").is_ok() && validate_size("512MiB").is_ok());
        assert!(validate_size("2GB").is_err() && validate_size("GiB").is_err());
        assert!(validate_env_key("APP_KEY").is_ok());
        assert!(validate_env_key("app_key").is_err() && validate_env_key("1A").is_err());
        assert!(validate_project_name("life-os").is_ok());
        assert!(validate_project_name("Life OS").is_err() && validate_project_name("-x").is_err());
        assert!(validate_tunnel_name("life-os").is_ok() && validate_tunnel_name("a b").is_err());
        assert!(
            validate_domain("life-os.bangkila.com").is_ok() && validate_domain("nodot").is_err()
        );
        assert!(validate_path("project", "/Users/x/Code/app").is_ok());
        assert!(validate_path("project", "relative").is_err());
        assert!(validate_path("project", "/a/../b").is_err());
        assert!(validate_path("project", "/a/$(x)").is_err());
    }
}
