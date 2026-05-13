use std::fs;

use colored::Colorize;

use crate::commands::lifecycle::{LaunchAgentState, service_state};
use crate::config::{ProjectConfig, bunker_home};
use crate::output;

pub fn run() -> anyhow::Result<()> {
    let home = bunker_home();

    if !home.exists() {
        output::warn("No bunkered projects.");
        return Ok(());
    }

    let mut found = false;

    println!();
    println!(
        "  {} {} {} {}",
        format!("{:<20}", "APP").bold(),
        format!("{:<8}", "PORT").bold(),
        format!("{:<10}", "STATUS").bold(),
        "DOMAIN".bold()
    );
    println!("  {:<20} {:<8} {:<10} ------", "---", "----", "------");

    let mut entries: Vec<_> = fs::read_dir(&home)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("bunker.conf").exists())
        .collect();

    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let dir_name = entry.file_name().to_string_lossy().to_string();

        let config = match ProjectConfig::load(&dir_name) {
            Ok(c) => c,
            Err(_) => continue, // skip invalid configs
        };

        let server_label = format!("com.bunker.{}.server", config.project_name);

        let status = match service_state(&server_label) {
            LaunchAgentState::Running(_) => format!("{:<10}", "running").green(),
            LaunchAgentState::Stopped => format!("{:<10}", "stopped").red(),
            LaunchAgentState::Unloaded => format!("{:<10}", "stopped").red(),
        };

        println!(
            "  {:<20} {:<8} {} {}",
            config.project_name, config.port, status, config.domain
        );
        found = true;
    }

    println!();

    if !found {
        output::warn("No bunkered projects.");
    }

    Ok(())
}
