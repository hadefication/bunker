//! Renders the guest's `shared/.env`: managed keys, then `[env]`, then in-app secrets.
use crate::config::{Config, SecretKind};
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use crate::secrets::store::SecretStore;
use zeroize::Zeroizing;

/// Keys a plain setting may never set.
pub const PROTECTED: [&str; 6] = [
    "APP_ENV",
    "APP_KEY",
    "DB_PASSWORD",
    "DB_USERNAME",
    "DB_DATABASE",
    "DB_HOST",
];
/// Managed keys a secret may never set either (APP_KEY and DB_PASSWORD are secrets by design).
pub const MANAGED_FIXED: [&str; 4] = ["APP_ENV", "DB_USERNAME", "DB_DATABASE", "DB_HOST"];

/// `"…"` with `\`, `"` and `$` escaped; newlines stay literal (phpdotenv reads multi-line quoted values).
pub fn quote(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One line per key, each key once (phpdotenv keeps the first definition): managed defaults,
/// overridden by `[env]` except for protected keys, then in-app secrets.
pub fn render_env(
    project: &str,
    cfg: &Config,
    store: &dyn SecretStore,
) -> Result<Zeroizing<String>> {
    let mut entries: Vec<(String, Zeroizing<String>)> = Vec::new();
    let mut put = |k: &str, v: Zeroizing<String>| match entries.iter_mut().find(|(ek, _)| ek == k) {
        Some(slot) => slot.1 = v,
        None => entries.push((k.to_string(), v)),
    };
    let plain = |s: &str| Zeroizing::new(s.to_string());
    put("APP_NAME", plain(project));
    put("APP_ENV", plain("production"));
    put("APP_DEBUG", plain("false"));
    if let Some(d) = &cfg.domain {
        put("APP_URL", plain(&format!("https://{d}")));
    }
    for (k, v) in [
        ("DB_CONNECTION", "mysql"),
        ("DB_HOST", "127.0.0.1"),
        ("DB_PORT", "3306"),
        ("DB_DATABASE", "app"),
        ("DB_USERNAME", "app"),
        ("LOG_CHANNEL", "daily"),
    ] {
        put(k, plain(v));
    }
    for (k, v) in &cfg.env {
        if !PROTECTED.contains(&k.as_str()) {
            put(k, plain(v));
        }
    }
    for (k, meta) in &cfg.secrets {
        if meta.kind != SecretKind::InApp || MANAGED_FIXED.contains(&k.as_str()) {
            continue;
        }
        let v = store.get(project, k)?.ok_or_else(|| {
            BunkerError::new(
                "secret_missing",
                ExitCode::Failure,
                format!("{k} is listed in [secrets] but has no value in the Keychain"),
            )
            .hint(format!("bunker env set {k} {project}"))
        })?;
        let v = String::from_utf8(v.to_vec()).map_err(|_| {
            BunkerError::new(
                "secret_not_utf8",
                ExitCode::Failure,
                format!("{k} is not valid UTF-8"),
            )
        })?;
        put(k, Zeroizing::new(v));
    }
    let mut out = Zeroizing::new(String::new());
    for (k, v) in &entries {
        out.push_str(k);
        out.push('=');
        out.push_str(&quote(v));
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SecretKind, SecretMeta};
    use crate::secrets::store::MemoryStore;

    fn cfg_with(secrets: &[&str], env: &[(&str, &str)]) -> Config {
        let mut c = Config::parse(&crate::config::fixture_config()).unwrap();
        for s in secrets {
            c.secrets.insert(
                s.to_string(),
                SecretMeta {
                    kind: SecretKind::InApp,
                    host: None,
                },
            );
        }
        for (k, v) in env {
            c.env.insert(k.to_string(), v.to_string());
        }
        c
    }

    #[test]
    fn render_env_quotes_special_characters() {
        let store = MemoryStore::default();
        store.set("p", "API_TOKEN", br#"a"b$c\d#e"#).unwrap();
        let out = render_env("p", &cfg_with(&["API_TOKEN"], &[]), &store).unwrap();
        assert!(
            out.lines().any(|l| l == r#"API_TOKEN="a\"b\$c\\d#e""#),
            "{}",
            out.as_str()
        );
    }

    #[test]
    fn render_env_keeps_multiline_pem() {
        let pem = "-----BEGIN PRIVATE KEY-----\nMIIabc+/=\nxyz\n-----END PRIVATE KEY-----\n";
        let store = MemoryStore::default();
        store
            .set("p", "PASSPORT_PRIVATE_KEY", pem.as_bytes())
            .unwrap();
        let out = render_env("p", &cfg_with(&["PASSPORT_PRIVATE_KEY"], &[]), &store).unwrap();
        assert!(
            out.contains(&format!("PASSPORT_PRIVATE_KEY=\"{pem}\"\n")),
            "{}",
            out.as_str()
        );
    }

    #[test]
    fn render_env_sets_production_flags() {
        let out = render_env(
            "life-os",
            &cfg_with(&[], &[("MAIL_MAILER", "log"), ("APP_NAME", "Life OS")]),
            &MemoryStore::default(),
        )
        .unwrap();
        let lines: Vec<&str> = out.lines().collect();
        for want in [
            "APP_ENV=\"production\"",
            "APP_DEBUG=\"false\"",
            "APP_URL=\"https://life-os.bangkila.com\"",
            "DB_HOST=\"127.0.0.1\"",
            "DB_DATABASE=\"app\"",
            "DB_USERNAME=\"app\"",
            "MAIL_MAILER=\"log\"",
            "APP_NAME=\"Life OS\"",
        ] {
            assert!(lines.contains(&want), "missing {want}:\n{}", out.as_str());
        }
        assert_eq!(
            lines.iter().filter(|l| l.starts_with("APP_NAME=")).count(),
            1,
            "phpdotenv keeps the first; no duplicates"
        );
    }

    #[test]
    fn missing_secret_value_is_an_error() {
        let e = render_env("p", &cfg_with(&["APP_KEY"], &[]), &MemoryStore::default()).unwrap_err();
        assert_eq!(e.code, "secret_missing");
        assert!(e.hint.unwrap().contains("bunker env set APP_KEY"));
    }
}
