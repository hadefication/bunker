use std::env;
use std::fs;
use std::os::unix::fs as unix_fs;
use std::process::Command;

use dialoguer::{Confirm, Input};

use crate::config::{launch_agents_dir, suggest_port, to_kebab, ProjectConfig};
use crate::framework::{self, FrameworkKind};
use crate::output;
use crate::templates;

pub fn run() -> anyhow::Result<()> {
    output::info("Initializing bunker for this project...");
    println!();

    let project_path = env::current_dir()?;
    let project_path_str = project_path.display().to_string();

    // Detect framework
    let framework = framework::detect(&project_path)
        .ok_or_else(|| anyhow::anyhow!("No supported framework detected in this directory."))?;

    output::info(&format!("Detected {} project", framework.display_name()));

    // Project name
    let default_name = to_kebab(
        project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project"),
    );
    let project_name: String = Input::new()
        .with_prompt("Project name")
        .default(default_name)
        .interact_text()?;

    // Detect binaries
    let php_path = which::which("php")
        .map(|p| p.display().to_string())
        .ok();
    let php_path: String = Input::new()
        .with_prompt("PHP path")
        .default(php_path.unwrap_or_default())
        .interact_text()?;
    if php_path.is_empty() {
        anyhow::bail!("PHP not found. Install via Homebrew or Herd.");
    }

    let frankenphp_path = which::which("frankenphp")
        .map(|p| p.display().to_string())
        .ok();
    let frankenphp_path: String = Input::new()
        .with_prompt("FrankenPHP path")
        .default(frankenphp_path.unwrap_or_default())
        .interact_text()?;
    if frankenphp_path.is_empty() {
        anyhow::bail!("FrankenPHP not found. Install it first.");
    }

    let cloudflared_path = which::which("cloudflared")
        .map(|p| p.display().to_string())
        .ok();
    let cloudflared_path: String = Input::new()
        .with_prompt("cloudflared path")
        .default(cloudflared_path.unwrap_or_default())
        .interact_text()?;
    if cloudflared_path.is_empty() {
        anyhow::bail!("cloudflared not found. Install it first.");
    }

    // Port
    let suggested = suggest_port(8700);
    let port: u16 = Input::new()
        .with_prompt("Port")
        .default(suggested)
        .interact_text()?;

    // Tunnel name
    let tunnel_name: String = Input::new()
        .with_prompt("Tunnel name")
        .default(project_name.clone())
        .interact_text()?;

    // Scheduler (Laravel-specific)
    let scheduler_enabled = matches!(framework, FrameworkKind::Laravel)
        && Confirm::new()
            .with_prompt("Enable scheduled tasks (schedule:work)?")
            .default(false)
            .interact()?;

    // Create or reuse cloudflared tunnel
    output::info(&format!("Setting up cloudflared tunnel '{}'...", tunnel_name));

    let tunnel_uuid = get_or_create_tunnel(&cloudflared_path, &tunnel_name)?;
    output::success(&format!("Tunnel ready: {}", tunnel_uuid));

    // Domain — defaults to the free cfargotunnel.com URL
    let cf_domain = format!("{}.cfargotunnel.com", tunnel_uuid);
    let domain: String = Input::new()
        .with_prompt("Domain")
        .default(cf_domain)
        .interact_text()?;

    // Build config
    let config = ProjectConfig {
        project_name: project_name.clone(),
        project_path: project_path_str,
        port,
        domain: domain.clone(),
        tunnel_name: tunnel_name.clone(),
        tunnel_uuid: tunnel_uuid.clone(),
        php_path,
        frankenphp_path,
        cloudflared_path: cloudflared_path.clone(),
        scheduler_enabled,
        framework,
    };

    // Write config
    config.write()?;

    // Generate Caddyfile
    let caddyfile_content = templates::caddyfile(&config);
    fs::write(config.project_dir().join("Caddyfile"), caddyfile_content)?;

    // Generate plists
    let plists = templates::generate_plists(&config);
    let la_dir = launch_agents_dir();
    fs::create_dir_all(&la_dir)?;

    for (filename, content) in &plists {
        let plist_path = config.project_dir().join(filename);
        fs::write(&plist_path, content)?;

        // Symlink to LaunchAgents
        let link_path = la_dir.join(filename);
        if link_path.exists() || link_path.is_symlink() {
            fs::remove_file(&link_path)?;
        }
        unix_fs::symlink(&plist_path, &link_path)?;
    }

    // Route DNS for custom domains
    let is_custom_domain = !domain.ends_with(".cfargotunnel.com");
    if is_custom_domain {
        output::info(&format!("Routing DNS for {}...", domain));
        let dns_result = Command::new(&cloudflared_path)
            .args(["tunnel", "route", "dns", &tunnel_name, &domain])
            .output();

        match dns_result {
            Ok(out) if out.status.success() => {
                output::success(&format!("DNS route created: {} -> tunnel", domain));
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if stderr.contains("already exists") {
                    output::success(&format!("DNS route already exists for {}", domain));
                } else {
                    output::warn(&format!(
                        "Could not route DNS automatically: {}",
                        stderr.trim()
                    ));
                    output::warn("Add a CNAME record manually:");
                    println!(
                        "  {} -> {}.cfargotunnel.com",
                        domain, tunnel_uuid
                    );
                }
            }
            Err(e) => {
                output::warn(&format!("DNS routing failed: {}", e));
                output::warn("Add a CNAME record manually:");
                println!(
                    "  {} -> {}.cfargotunnel.com",
                    domain, tunnel_uuid
                );
            }
        }
    }

    println!();
    output::success(&format!("Bunker initialized for {}!", project_name));
    println!();
    output::info(&format!("Config:  {}", config.project_dir().display()));
    output::info(&format!("Server:  localhost:{}", port));
    output::info(&format!("Domain:  {}", domain));
    println!();
    output::warn("Next steps:");
    println!("  1. Create .env.production in your project (if needed)");
    println!("  2. Run: bunker start");

    Ok(())
}

fn get_or_create_tunnel(cloudflared: &str, name: &str) -> anyhow::Result<String> {
    // Try to find existing tunnel
    let output = Command::new(cloudflared)
        .args(["tunnel", "list", "--name", name, "--output", "json"])
        .output()?;

    if output.status.success() {
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
        if let Some(arr) = json.as_array() {
            if let Some(first) = arr.first() {
                if let Some(id) = first.get("id").and_then(|v| v.as_str()) {
                    return Ok(id.to_string());
                }
            }
        }
    }

    // Create new tunnel
    output::info(&format!("Creating new tunnel '{}'...", name));
    let output = Command::new(cloudflared)
        .args(["tunnel", "create", name])
        .output()?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Extract UUID from output
    let uuid_re =
        regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")?;
    let uuid = uuid_re
        .find(&combined)
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| anyhow::anyhow!("Failed to create tunnel. Check cloudflared auth."))?;

    Ok(uuid)
}
