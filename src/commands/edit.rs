use std::env;
use std::process::Command;

use crate::config::{resolve_project, ProjectConfig};
use crate::output;

pub fn run(project: Option<String>) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;
    let project_dir = config.project_dir();
    let editor = env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());

    output::info(&format!(
        "Opening {} in {}...",
        project_dir.display(),
        editor
    ));

    let mut parts = editor.split_whitespace();
    let bin = parts.next().unwrap_or("vim");
    let status = Command::new(bin).args(parts).arg(&project_dir).status()?;

    if !status.success() {
        anyhow::bail!("Editor exited with non-zero status");
    }

    Ok(())
}
