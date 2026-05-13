use std::net::{TcpStream, ToSocketAddrs};
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
    println!("  {:<12} {:<10} ---", "-------", "-----");

    for label in config.service_labels() {
        let service_name = label.rsplit('.').next().unwrap_or(&label);

        let service = service_state(&label);

        let (state, pid) = match service {
            LaunchAgentState::Running(pid) => {
                (format!("{:<10}", "running").green(), format!("{}", pid))
            }
            LaunchAgentState::Stopped => (format!("{:<10}", "stopped").red(), "-".to_string()),
            LaunchAgentState::Unloaded => (format!("{:<10}", "unloaded").dimmed(), "-".to_string()),
        };

        println!("  {:<12} {} {}", service_name, state, pid);
    }
    println!();

    // Health check — only attempt TCP connect if the server service is running
    let server_label = format!("com.bunker.{}.server", config.project_name);
    let server_running = matches!(
        service_state(&server_label),
        LaunchAgentState::Running(_) | LaunchAgentState::Stopped
    );

    let health = if server_running {
        if is_port_reachable(config.port) {
            format!("{} on port {}", "reachable".green(), config.port)
        } else {
            format!("{} on port {}", "unreachable".red(), config.port)
        }
    } else {
        format!("{} on port {}", "—".dimmed(), config.port)
    };
    println!("  Health:  {}", health);
    println!("  Domain:  {}", config.domain);
    println!();

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchAgentState {
    Running(u32),
    Stopped,
    Unloaded,
}

pub fn service_state(label: &str) -> LaunchAgentState {
    let Ok(uid_output) = Command::new("id").arg("-u").output() else {
        return LaunchAgentState::Unloaded;
    };

    if !uid_output.status.success() {
        return LaunchAgentState::Unloaded;
    }

    let uid = String::from_utf8_lossy(&uid_output.stdout)
        .trim()
        .to_string();
    let target = format!("gui/{}/{}", uid, label);
    let Ok(output) = Command::new("launchctl").args(["print", &target]).output() else {
        return LaunchAgentState::Unloaded;
    };

    if !output.status.success() {
        return LaunchAgentState::Unloaded;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_launchctl_print(&stdout)
}

fn parse_launchctl_print(output: &str) -> LaunchAgentState {
    if let Some(pid) = extract_pid(output) {
        return LaunchAgentState::Running(pid);
    }

    LaunchAgentState::Stopped
}

fn extract_pid(launchctl_output: &str) -> Option<u32> {
    for line in launchctl_output.lines() {
        let line = line.trim();
        if line.starts_with("\"PID\"") || line.starts_with("pid =") || line.contains("PID") {
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

pub fn is_port_reachable(port: u16) -> bool {
    let Ok(addrs) = ("localhost", port).to_socket_addrs() else {
        return false;
    };

    addrs
        .into_iter()
        .any(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok())
}

#[cfg(test)]
mod tests {
    use super::{LaunchAgentState, extract_pid, parse_launchctl_print};

    #[test]
    fn extracts_pid_from_launchctl_print_output() {
        let output = r#"
gui/501/com.bunker.life-os.server = {
    state = running
    pid = 1686
}
"#;

        assert_eq!(extract_pid(output), Some(1686));
        assert_eq!(
            parse_launchctl_print(output),
            LaunchAgentState::Running(1686)
        );
    }

    #[test]
    fn parses_loaded_agent_without_pid_as_stopped() {
        let output = r#"
gui/501/com.bunker.life-os.queue = {
    state = waiting
}
"#;

        assert_eq!(parse_launchctl_print(output), LaunchAgentState::Stopped);
    }
}
