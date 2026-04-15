use std::process::Command;

use crate::output;

const REPO: &str = "hadefication/bunker";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run() -> anyhow::Result<()> {
    output::info(&format!("Current version: {}", CURRENT_VERSION));
    output::info("Checking for updates...");

    let latest = fetch_latest_version()?;
    let latest_tag = latest.trim_start_matches('v');

    if latest_tag == CURRENT_VERSION {
        output::success("Already up to date.");
        return Ok(());
    }

    output::info(&format!("New version available: {}", latest_tag));
    output::info("Running install script...");

    let url = format!("https://raw.githubusercontent.com/{}/main/install.sh", REPO);

    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("curl -fsSL '{}' | sh", url))
        .status()?;

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
