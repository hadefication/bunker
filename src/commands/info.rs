use std::net::TcpStream;
use std::process::Command;
use std::time::Duration;

use colored::Colorize;

use crate::config::{ProjectConfig, resolve_project};

pub fn run(project: Option<String>, verbose: bool) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;

    let server_label = format!("com.bunker.{}.server", config.project_name);
    let is_running = Command::new("launchctl")
        .args(["list", &server_label])
        .output()
        .is_ok_and(|o| o.status.success());

    let healthy = is_running
        && TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", config.port).parse().unwrap(),
            Duration::from_secs(2),
        )
        .is_ok();

    let status = if is_running {
        "running".green()
    } else {
        "stopped".red()
    };

    let health = if !is_running {
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
