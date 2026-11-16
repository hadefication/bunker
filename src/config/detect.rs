//! What the repo is: Laravel or not, its PHP constraint, extensions, Node and Passport.
use crate::config::Stack;
use crate::config::constraint::php_from_constraint;
use crate::config::validate::validate_extension;
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub stack: Stack,
    /// The highest supported PHP the constraint allows; `None` when absent or unreadable.
    pub php: Option<String>,
    /// `require.php` as written, so `init` can explain why `php` is `None`.
    pub php_constraint: Option<String>,
    pub extensions: Vec<String>,
    pub node: Option<String>,
    pub passport: bool,
}

/// Laravel means `artisan` exists and `composer.json` requires `laravel/framework`.
pub fn detect(repo: &Path) -> Result<Detected> {
    let composer = read_json(&repo.join("composer.json"))?;
    let require = composer.get("require").and_then(Value::as_object);
    let requires = |pkg: &str| require.is_some_and(|r| r.contains_key(pkg));
    if !repo.join("artisan").is_file() || !requires("laravel/framework") {
        return Err(BunkerError::new(
            "unsupported_stack",
            ExitCode::Refused,
            format!(
                "{} is not a Laravel app (bunker needs artisan and laravel/framework in composer.json)",
                repo.display()
            ),
        ));
    }
    let php_constraint = require
        .and_then(|r| r.get("php"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let php = php_constraint
        .as_deref()
        .and_then(|c| php_from_constraint(c).ok());
    let mut extensions: Vec<String> = require
        .into_iter()
        .flat_map(|r| r.keys())
        .filter_map(|k| k.strip_prefix("ext-"))
        .map(str::to_ascii_lowercase)
        .filter(|e| validate_extension(e).is_ok())
        .collect();
    extensions.sort();
    extensions.dedup();
    let package = read_json(&repo.join("package.json")).unwrap_or(Value::Null);
    let has_build = package
        .get("scripts")
        .and_then(|s| s.get("build"))
        .is_some();
    Ok(Detected {
        stack: Stack::Laravel,
        php,
        php_constraint,
        extensions,
        node: has_build.then(|| "22".to_string()),
        passport: requires("laravel/passport"),
    })
}

/// A missing file reads as `null`; an unreadable or malformed one is an error.
fn read_json(path: &Path) -> Result<Value> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            BunkerError::new(
                "invalid_json",
                ExitCode::Failure,
                format!("cannot parse {}: {e}", path.display()),
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(BunkerError::io(
            format!("cannot read {}", path.display()),
            e,
        )),
    }
}

#[cfg(test)]
pub(crate) fn laravel_fixture(dir: &Path) {
    use std::fs;
    fs::write(dir.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    fs::write(
        dir.join("composer.json"),
        r#"{"require":{"php":"^8.3","laravel/framework":"^12.0","laravel/passport":"^13.0","ext-intl":"*","ext-bcmath":"*"}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("package.json"),
        r#"{"scripts":{"build":"vite build","dev":"vite"}}"#,
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_laravel_passport_and_node() {
        let d = tempfile::tempdir().unwrap();
        laravel_fixture(d.path());
        let got = detect(d.path()).unwrap();
        assert_eq!(got.stack, Stack::Laravel);
        assert_eq!(got.php.as_deref(), Some("8.5"));
        assert_eq!(got.extensions, ["bcmath", "intl"]);
        assert_eq!(got.node.as_deref(), Some("22"));
        assert!(got.passport);
    }

    #[test]
    fn no_build_script_means_no_node_and_no_passport() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("artisan"), "").unwrap();
        fs::write(
            d.path().join("composer.json"),
            r#"{"require":{"php":"dev-main","laravel/framework":"^12"}}"#,
        )
        .unwrap();
        fs::write(
            d.path().join("package.json"),
            r#"{"scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        let got = detect(d.path()).unwrap();
        assert_eq!(
            (got.php, got.php_constraint.as_deref()),
            (None, Some("dev-main"))
        );
        assert_eq!((got.node, got.passport), (None, false));
    }

    #[test]
    fn non_laravel_repo_is_refused() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("composer.json"),
            r#"{"require":{"laravel/framework":"^12"}}"#,
        )
        .unwrap();
        let e = detect(d.path()).unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("unsupported_stack", crate::output::ExitCode::Refused)
        );
    }
}
