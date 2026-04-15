use std::process::Command;

use crate::output;

const REPO: &str = "hadefication/bunker";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run() -> anyhow::Result<()> {
    output::info(&format!("Current version: {}", CURRENT_VERSION));
    output::info("Checking for updates...");

    let latest = fetch_latest_version()?;
    let latest_tag = latest.trim_start_matches('v');

    if !is_newer(latest_tag, CURRENT_VERSION) {
        output::success("Already up to date.");
        return Ok(());
    }

    output::info(&format!("New version available: {}", latest_tag));
    output::info("Downloading install script...");

    let url = format!("https://raw.githubusercontent.com/{}/main/install.sh", REPO);

    let tmp = std::env::temp_dir().join("bunker-install.sh");
    let download = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&tmp)
        .arg(&url)
        .status()?;

    if !download.success() {
        anyhow::bail!("Failed to download install script.");
    }

    let status = Command::new("sh").arg(&tmp).status()?;
    let _ = std::fs::remove_file(&tmp);

    if !status.success() {
        anyhow::bail!("Update failed.");
    }

    Ok(())
}

fn fetch_latest_version() -> anyhow::Result<String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            &format!("https://api.github.com/repos/{}/releases/latest", REPO),
        ])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("Failed to check for updates. Are you online?");
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Could not parse latest release from GitHub."))?;

    Ok(tag.to_string())
}

/// Returns true if `latest` is strictly newer than `current` (semver comparison)
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Option<(u32, u32, u32)> {
        let mut parts = s.split('.');
        Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ))
    };
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false, // unparseable = don't update
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_version() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.1", "0.1.0"));
    }

    #[test]
    fn same_version() {
        assert!(!is_newer("0.1.0", "0.1.0"));
    }

    #[test]
    fn older_version() {
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("0.1.0", "1.0.0"));
    }

    #[test]
    fn unparseable_version() {
        assert!(!is_newer("abc", "0.1.0"));
        assert!(!is_newer("0.1.0", "xyz"));
    }
}
