use std::fs;
use std::io::IsTerminal;
use std::process::Command;

use dialoguer::Confirm;

use crate::commands::lifecycle;
use crate::config::{ProjectConfig, launch_agents_dir, resolve_project};
use crate::output;

fn confirm_or(label: &str, default: bool, yes: bool) -> anyhow::Result<bool> {
    if yes {
        return Ok(true);
    }
    if std::io::stdin().is_terminal() {
        Ok(Confirm::new()
            .with_prompt(label)
            .default(default)
            .interact()?)
    } else {
        Ok(default)
    }
}

pub fn run(project: Option<String>, yes: bool) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;
    let project_dir = config.project_dir();

    // Stop first
    lifecycle::stop(Some(name.clone()))?;

    // Remove symlinks
    output::info("Removing LaunchAgent symlinks...");
    let la_dir = launch_agents_dir();
    for label in config.service_labels() {
        let link = la_dir.join(format!("{}.plist", label));
        if link.exists() || link.is_symlink() {
            fs::remove_file(&link)?;
        }
    }

    // Remind user to remove DNS record for custom domains.
    // cloudflared CLI can create CNAME records but has no way to delete them —
    // deletion requires the Cloudflare API or dashboard.
    let is_custom_domain = !config.domain.ends_with(".cfargotunnel.com");
    if is_custom_domain {
        let zone = config.domain
            .splitn(2, '.')
            .nth(1)
            .unwrap_or(&config.domain);
        output::warn(&format!(
            "The CNAME record for {} must be removed manually.", config.domain
        ));
        output::info(&format!(
            "  → Cloudflare Dashboard > {} > DNS > Records", zone
        ));
        output::info(&format!(
            "  → Find the CNAME entry for '{}' and delete it", config.domain
        ));
        output::info("  Leaving it will show a Cloudflare Tunnel error page to visitors.");
    }

    // Delete cloudflared tunnel
    let delete_tunnel = confirm_or(
        &format!("Delete cloudflared tunnel '{}'?", config.tunnel_name),
        true,
        yes,
    )?;

    if delete_tunnel {
        output::info(&format!("Deleting tunnel '{}'...", config.tunnel_name));
        let result = Command::new(&config.cloudflared_path)
            .args(["tunnel", "delete", &config.tunnel_name])
            .output();

        match result {
            Ok(out) if out.status.success() => {
                output::success(&format!("Tunnel '{}' deleted", config.tunnel_name));
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                output::warn(&format!("Could not delete tunnel: {}", stderr.trim()));
            }
            Err(e) => {
                output::warn(&format!("Could not delete tunnel: {}", e));
            }
        }
    }

    // Ask before nuking config
    let remove = confirm_or(
        &format!("Remove all config in {}?", project_dir.display()),
        false,
        yes,
    )?;

    if remove {
        fs::remove_dir_all(&project_dir)?;
        output::success(&format!("Removed {}", project_dir.display()));
    } else {
        output::info(&format!("Config preserved at {}", project_dir.display()));
    }

    Ok(())
}
