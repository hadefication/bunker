use std::process::Command;

use crate::config::{launch_agents_dir, resolve_project, ProjectConfig};
use crate::output;

pub fn start(project: Option<String>) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;

    output::info(&format!("Starting {}...", name));

    let la_dir = launch_agents_dir();
    for label in config.service_labels() {
        let plist = la_dir.join(format!("{}.plist", label));
        if plist.exists() {
            let _ = Command::new("launchctl")
                .args(["load", "-w"])
                .arg(&plist)
                .output();
        }
    }

    output::success(&format!("Started {}", name));
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
            let _ = Command::new("launchctl").arg("unload").arg(&plist).output();
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
        "  {:<12} {:<10} {}",
        "\x1b[1mSERVICE\x1b[0m",
        "\x1b[1mSTATE\x1b[0m",
        "\x1b[1mPID\x1b[0m"
    );
    println!("  {:<12} {:<10} {}", "-------", "-----", "---");

    for label in config.service_labels() {
        let service_name = label.rsplit('.').next().unwrap_or(&label);

        let output = Command::new("launchctl")
            .args(["list", &label])
            .output();

        let (state, pid) = match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let pid = extract_pid(&stdout);
                if let Some(p) = pid {
                    ("\x1b[32mrunning\x1b[0m", format!("{}", p))
                } else {
                    ("\x1b[31mstopped\x1b[0m", "-".to_string())
                }
            }
            _ => ("\x1b[2munloaded\x1b[0m", "-".to_string()),
        };

        println!("  {:<12} {:<20} {}", service_name, state, pid);
    }
    println!();

    Ok(())
}

fn extract_pid(launchctl_output: &str) -> Option<u32> {
    for line in launchctl_output.lines() {
        let line = line.trim();
        if line.starts_with("\"PID\"") || line.contains("PID") {
            // launchctl list <label> outputs key-value pairs
            if let Some(val) = line.split('=').nth(1).or_else(|| line.split_whitespace().last()) {
                let val = val.trim().trim_end_matches(';').trim_matches('"');
                if let Ok(pid) = val.parse::<u32>() {
                    if pid > 0 {
                        return Some(pid);
                    }
                }
            }
        }
    }
    None
}
