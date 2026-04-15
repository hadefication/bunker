use std::fs;

use crate::config::{launch_agents_dir, resolve_project, ProjectConfig};
use crate::output;
use crate::templates;

pub fn run(project: Option<String>) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;

    output::info(&format!("Updating configs for {}...", name));

    // Re-generate Caddyfile
    let caddyfile_content = templates::caddyfile(&config);
    crate::config::write_restricted(&config.project_dir().join("Caddyfile"), &caddyfile_content)?;
    output::success("Caddyfile updated");

    // Re-generate cloudflared config
    let cf_config = templates::cloudflared_config(&config);
    crate::config::write_restricted(&config.project_dir().join("cloudflared.yml"), &cf_config)?;
    output::success("cloudflared.yml updated");

    // Re-generate plists
    let plists = templates::generate_plists(&config);
    let la_dir = launch_agents_dir();
    fs::create_dir_all(&la_dir)?;

    for (filename, content) in &plists {
        let plist_path = config.project_dir().join(filename);
        crate::config::write_restricted(&plist_path, content)?;

        let link_path = la_dir.join(filename);
        crate::config::atomic_symlink(&plist_path, &link_path)?;
    }
    output::success(&format!("{} plist(s) updated", plists.len()));

    println!();
    output::success(&format!("Updated {}. Restart to apply changes.", name));

    Ok(())
}
