use colored::Colorize;

use crate::commands::lifecycle::{LaunchAgentState, is_port_reachable, service_state};
use crate::config::{ProjectConfig, resolve_project};

pub fn run(project: Option<String>, verbose: bool) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;

    let server_label = format!("com.bunker.{}.server", config.project_name);
    let state = service_state(&server_label);
    let is_loaded = matches!(
        state,
        LaunchAgentState::Running(_) | LaunchAgentState::Stopped
    );
    let healthy = is_loaded && is_port_reachable(config.port);

    let status = match state {
        LaunchAgentState::Running(_) => "running".green(),
        LaunchAgentState::Stopped | LaunchAgentState::Unloaded => "stopped".red(),
    };

    let health = if !is_loaded {
        "—".dimmed()
    } else if healthy {
        "reachable".green()
    } else {
        "unreachable".red()
    };

    let tunnel_id = if verbose {
        config.tunnel_uuid.clone()
    } else {
        format!("{}…", &config.tunnel_uuid[..8])
    };

    println!();
    println!("  {}", config.project_name.bold());
    println!();
    println!("  {:<16} {}", "Status:", status);
    println!("  {:<16} {}", "Health:", health);
    println!("  {:<16} {}", "Domain:", config.domain);
    println!("  {:<16} {}", "Port:", config.port);
    println!("  {:<16} {}", "Path:", config.project_path);
    println!("  {:<16} {}", "Framework:", config.framework.as_str());
    println!("  {:<16} {}", "Tunnel:", config.tunnel_name);
    println!("  {:<16} {}", "Tunnel ID:", tunnel_id);
    if verbose {
        println!("  {:<16} {}", "PHP:", config.php_path);
        println!("  {:<16} {}", "FrankenPHP:", config.frankenphp_path);
        println!("  {:<16} {}", "cloudflared:", config.cloudflared_path);
    }
    println!(
        "  {:<16} {}",
        "Scheduler:",
        if config.scheduler_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!("  {:<16} {}", "Config:", config.conf_path().display());
    println!("  {:<16} {}", "Logs:", config.logs_dir().display());
    println!();

    Ok(())
}
