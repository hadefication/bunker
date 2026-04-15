use std::env;
use std::process::Command;

use crate::config::{ProjectConfig, resolve_project};
use crate::output;

pub fn run(project: Option<String>) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;
    let conf_path = config.conf_path();
    let editor = env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());

    let mut parts = editor.split_whitespace();
    let bin = parts.next().unwrap_or("vim");

    if which::which(bin).is_err() {
        anyhow::bail!("Editor '{}' not found. Set $EDITOR to a valid binary.", bin);
    }

    output::info(&format!("Opening {} in {}...", conf_path.display(), editor));

    let status = Command::new(bin).args(parts).arg(&conf_path).status()?;

    if !status.success() {
        anyhow::bail!("Editor exited with non-zero status");
    }

    Ok(())
}
