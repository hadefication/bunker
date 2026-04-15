use std::env;
use std::fs;
use std::io::IsTerminal;
use std::os::unix::fs as unix_fs;
use std::process::Command;

use dialoguer::{Confirm, Input};

use crate::config::{launch_agents_dir, suggest_port, to_kebab, ProjectConfig};
use crate::framework::{self, FrameworkKind};
use crate::output;
use crate::templates;

pub struct InitArgs {
    pub name: Option<String>,
    pub port: Option<u16>,
    pub domain: Option<String>,
    pub tunnel: Option<String>,
    pub scheduler: bool,
    pub php: Option<String>,
    pub frankenphp: Option<String>,
    pub cloudflared: Option<String>,
    pub yes: bool,
    pub dry_run: bool,
}

fn prompt_or(label: &str, provided: Option<String>, default: Option<String>, yes: bool) -> anyhow::Result<String> {
    if let Some(val) = provided {
        return Ok(val);
    }

    if yes {
        return default.ok_or_else(|| anyhow::anyhow!("--{} is required in non-interactive mode", label));
    }

    if std::io::stdin().is_terminal() {
        let mut input = Input::new().with_prompt(label);
        if let Some(d) = default {
            input = input.default(d);
        }
        Ok(input.interact_text()?)
    } else {
        default.ok_or_else(|| anyhow::anyhow!(
            "No TTY available and --{} not provided. Use flags or --yes for non-interactive mode.",
            label.to_lowercase().replace(' ', "-")
        ))
    }
}

fn confirm_or(label: &str, default: bool, yes: bool, flag_value: bool) -> anyhow::Result<bool> {
    if yes {
        return Ok(flag_value);
    }

    if std::io::stdin().is_terminal() {
        Ok(Confirm::new()
            .with_prompt(label)
            .default(default)
            .interact()?)
    } else {
        Ok(flag_value)
    }
}

pub fn run(args: InitArgs) -> anyhow::Result<()> {
    let dry_run = args.dry_run;

    if dry_run {
        output::info("[dry-run] Previewing bunker init...");
    } else {
        output::info("Initializing bunker for this project...");
    }
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
    let project_name = prompt_or("Project name", args.name, Some(default_name), args.yes || dry_run)?;
    crate::config::validate_project_name(&project_name)?;

    // Detect binaries
    let detected_php = which::which("php").map(|p| p.display().to_string()).ok();
    let php_path = prompt_or("PHP path", args.php, detected_php, args.yes || dry_run)?;
    if php_path.is_empty() {
        anyhow::bail!("PHP not found. Install via Homebrew or Herd.");
    }

    let detected_frankenphp = which::which("frankenphp").map(|p| p.display().to_string()).ok();
    let frankenphp_path = prompt_or("FrankenPHP path", args.frankenphp, detected_frankenphp, args.yes || dry_run)?;
    if frankenphp_path.is_empty() {
        anyhow::bail!("FrankenPHP not found. Install it first.");
    }

    let detected_cloudflared = which::which("cloudflared").map(|p| p.display().to_string()).ok();
    let cloudflared_path = prompt_or("cloudflared path", args.cloudflared, detected_cloudflared, args.yes || dry_run)?;
    if cloudflared_path.is_empty() {
        anyhow::bail!("cloudflared not found. Install it first.");
    }

    // Port
    let suggested = suggest_port(8700);
    let port: u16 = if let Some(p) = args.port {
        p
    } else if args.yes || dry_run {
        suggested
    } else if std::io::stdin().is_terminal() {
        Input::new()
            .with_prompt("Port")
            .default(suggested)
            .interact_text()?
    } else {
        suggested
    };

    // Tunnel name
    let tunnel_name = prompt_or("Tunnel name", args.tunnel, Some(project_name.clone()), args.yes || dry_run)?;
    crate::config::validate_tunnel_name(&tunnel_name)?;

    // Scheduler (Laravel-specific)
    let scheduler_enabled = matches!(framework, FrameworkKind::Laravel)
        && confirm_or(
            "Enable scheduled tasks (schedule:work)?",
            false,
            args.yes || dry_run,
            args.scheduler,
        )?;

    // Tunnel + domain
    let (tunnel_uuid, domain) = if dry_run {
        let uuid = "<tunnel-uuid>".to_string();
        let domain = args.domain.unwrap_or_else(|| format!("{}.cfargotunnel.com", uuid));
        if !domain.ends_with(".cfargotunnel.com") {
            crate::config::validate_domain(&domain)?;
        }
        (uuid, domain)
    } else {
        output::info(&format!("Setting up cloudflared tunnel '{}'...", tunnel_name));
        let uuid = get_or_create_tunnel(&cloudflared_path, &tunnel_name)?;
        output::success(&format!("Tunnel ready: {}", uuid));

        let cf_domain = format!("{}.cfargotunnel.com", uuid);
        let domain = prompt_or("Domain", args.domain, Some(cf_domain), args.yes)?;
        crate::config::validate_domain(&domain)?;
        (uuid, domain)
    };

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

    if dry_run {
        println!();
        output::info("[dry-run] Would generate the following:");
        println!();

        println!("  Config dir:  {}", config.project_dir().display());
        println!("  Logs dir:    {}", config.logs_dir().display());
        println!();

        println!("  --- bunker.conf ---");
        println!("  PROJECT_NAME=\"{}\"", config.project_name);
        println!("  PROJECT_PATH=\"{}\"", config.project_path);
        println!("  PORT={}", config.port);
        println!("  DOMAIN=\"{}\"", config.domain);
        println!("  TUNNEL_NAME=\"{}\"", config.tunnel_name);
        println!("  TUNNEL_UUID=\"{}\"", config.tunnel_uuid);
        println!("  SCHEDULER_ENABLED=\"{}\"", config.scheduler_enabled);
        println!("  FRAMEWORK=\"{}\"", config.framework.as_str());
        println!();

        println!("  --- Caddyfile ---");
        for line in templates::caddyfile(&config).lines() {
            println!("  {}", line);
        }
        println!();

        println!("  --- cloudflared.yml ---");
        for line in templates::cloudflared_config(&config).lines() {
            println!("  {}", line);
        }
        println!();

        let plists = templates::generate_plists(&config);
        println!("  --- Plists ({}) ---", plists.len());
        for (filename, _) in &plists {
            println!("  {}", filename);
            println!("    -> symlink to {}/{}", launch_agents_dir().display(), filename);
        }
        println!();

        let is_custom_domain = !domain.ends_with(".cfargotunnel.com");
        if is_custom_domain {
            output::info(&format!("[dry-run] Would route DNS: {} -> tunnel", domain));
        }

        println!();
        output::info("[dry-run] No files written, no tunnel created.");
        return Ok(());
    }

    // Write config
    config.write()?;

    // Generate Caddyfile
    let caddyfile_content = templates::caddyfile(&config);
    crate::config::write_restricted(&config.project_dir().join("Caddyfile"), &caddyfile_content)?;

    // Generate cloudflared config
    let cf_config = templates::cloudflared_config(&config);
    crate::config::write_restricted(&config.project_dir().join("cloudflared.yml"), &cf_config)?;

    // Generate plists
    let plists = templates::generate_plists(&config);
    let la_dir = launch_agents_dir();
    fs::create_dir_all(&la_dir)?;

    for (filename, content) in &plists {
        let plist_path = config.project_dir().join(filename);
        crate::config::write_restricted(&plist_path, content)?;

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
            .args(["tunnel", "route", "dns", "-f", &tunnel_uuid, &domain])
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

    output::info(&format!("Creating new tunnel '{}'...", name));
    let output = Command::new(cloudflared)
        .args(["tunnel", "create", name])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create tunnel: {}", stderr.trim());
    }

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let uuid_re =
        regex::Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")?;
    let uuid = uuid_re
        .find(&combined)
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| anyhow::anyhow!("Tunnel created but could not extract UUID from output."))?;

    Ok(uuid)
}
