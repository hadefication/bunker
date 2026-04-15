use std::fs;
use std::process::Command;

use dialoguer::Confirm;

use crate::commands::lifecycle;
use crate::config::{launch_agents_dir, resolve_project, ProjectConfig};
use crate::output;

pub fn run(project: Option<String>) -> anyhow::Result<()> {
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

    // Clean up DNS route for custom domains
    let is_custom_domain = !config.domain.ends_with(".cfargotunnel.com");
    if is_custom_domain {
        output::info(&format!("Removing DNS route for {}...", config.domain));
        let dns_result = Command::new(&config.cloudflared_path)
            .args(["tunnel", "route", "dns", "--remove", &config.tunnel_name, &config.domain])
            .output();

        match dns_result {
            Ok(out) if out.status.success() => {
                output::success(&format!("DNS route removed for {}", config.domain));
            }
            _ => {
                output::warn(&format!(
                    "Could not remove DNS route automatically. Remove the CNAME for {} manually.",
                    config.domain
                ));
            }
        }
    }

    // Delete cloudflared tunnel
    let delete_tunnel = Confirm::new()
        .with_prompt(format!(
            "Delete cloudflared tunnel '{}'?",
            config.tunnel_name
        ))
        .default(true)
        .interact()?;

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
    let remove = Confirm::new()
        .with_prompt(format!("Remove all config in {}?", project_dir.display()))
        .default(false)
        .interact()?;

    if remove {
        fs::remove_dir_all(&project_dir)?;
        output::success(&format!("Removed {}", project_dir.display()));
    } else {
        output::info(&format!("Config preserved at {}", project_dir.display()));
    }

    Ok(())
}
