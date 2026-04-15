use std::net::TcpStream;
use std::process::Command;
use std::time::Duration;

use colored::Colorize;

use crate::config::{ProjectConfig, launch_agents_dir, resolve_project};
use crate::output;

pub fn start(project: Option<String>) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;

    output::info(&format!("Starting {}...", name));

    let la_dir = launch_agents_dir();
    let mut loaded = 0u32;
    let mut failed = Vec::new();

    for label in config.service_labels() {
        let plist = la_dir.join(format!("{}.plist", label));
        if plist.exists() {
            let result = Command::new("launchctl")
                .args(["load", "-w"])
                .arg(&plist)
                .output();

            match &result {
                Ok(out) if out.status.success() => loaded += 1,
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let service = label.rsplit('.').next().unwrap_or(&label);
                    output::warn(&format!("{}: {}", service, stderr.trim()));
                    failed.push(service.to_string());
                }
                Err(e) => {
                    let service = label.rsplit('.').next().unwrap_or(&label);
                    output::warn(&format!("{}: {}", service, e));
                    failed.push(service.to_string());
                }
            }
        }
    }

    if loaded == 0 {
        anyhow::bail!("No services loaded for {}. Run 'bunker init' first.", name);
    }

    if failed.is_empty() {
        output::success(&format!("Started {}", name));
    } else {
        output::warn(&format!(
            "Started {} ({} loaded, {} failed: {})",
            name,
            loaded,
            failed.len(),
            failed.join(", ")
        ));
    }

    status(Some(name))
}

pub fn stop(project: Option<String>) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;

    output::info(&format!("Stopping {}...", name));

    let la_dir = launch_agents_dir();
    for label in config.service_labels() {
        let plist = la_dir.join(format!("{}.plist", label));
        if plist.exists() {
            let result = Command::new("launchctl").arg("unload").arg(&plist).output();

            if let Ok(out) = &result
                && !out.status.success()
            {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let service = label.rsplit('.').next().unwrap_or(&label);
                output::warn(&format!("{}: {}", service, stderr.trim()));
            }
        }
    }

    output::success(&format!("Stopped {}", name));
    Ok(())
}

pub fn restart(project: Option<String>) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    stop(Some(name.clone()))?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    start(Some(name))
}

pub fn status(project: Option<String>) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;

    println!();
    println!(
        "  {} {} {}",
        format!("{:<12}", "SERVICE").bold(),
        format!("{:<10}", "STATE").bold(),
        "PID".bold()
    );
    println!(
        "  {:<12} {:<10} ---",
        "-------", "-----"
    );

    for label in config.service_labels() {
        let service_name = label.rsplit('.').next().unwrap_or(&label);

        let output = Command::new("launchctl").args(["list", &label]).output();

        let (state, pid) = match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let pid = extract_pid(&stdout);
                if let Some(p) = pid {
                    (format!("{:<10}", "running").green(), format!("{}", p))
                } else {
                    (format!("{:<10}", "stopped").red(), "-".to_string())
                }
            }
            _ => (format!("{:<10}", "unloaded").dimmed(), "-".to_string()),
        };

        println!("  {:<12} {} {}", service_name, state, pid);
    }
    println!();

    // Health check — only attempt TCP connect if the server service is running
    let server_label = format!("com.bunker.{}.server", config.project_name);
    let server_running = Command::new("launchctl")
        .args(["list", &server_label])
        .output()
        .is_ok_and(|o| o.status.success());

    let health = if server_running {
        match TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", config.port).parse().unwrap(),
            Duration::from_secs(2),
        ) {
            Ok(_) => format!("{} on port {}", "reachable".green(), config.port),
            Err(_) => format!("{} on port {}", "unreachable".red(), config.port),
        }
    } else {
        format!("{} on port {}", "—".dimmed(), config.port)
    };
    println!("  Health:  {}", health);
    println!("  Domain:  {}", config.domain);
    println!();

    Ok(())
}

fn extract_pid(launchctl_output: &str) -> Option<u32> {
    for line in launchctl_output.lines() {
        let line = line.trim();
        if line.starts_with("\"PID\"") || line.contains("PID") {
            // launchctl list <label> outputs key-value pairs
            if let Some(val) = line
                .split('=')
                .nth(1)
                .or_else(|| line.split_whitespace().last())
            {
                let val = val.trim().trim_end_matches(';').trim_matches('"');
                if let Ok(pid) = val.parse::<u32>()
                    && pid > 0
                {
                    return Some(pid);
                }
            }
        }
    }
    None
}
