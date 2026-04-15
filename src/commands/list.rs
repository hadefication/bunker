use std::fs;
use std::process::Command;

use crate::config::{bunker_home, ProjectConfig};
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
        "  {:<20} {:<8} {:<10} {}",
        "\x1b[1mPROJECT\x1b[0m",
        "\x1b[1mPORT\x1b[0m",
        "\x1b[1mSTATUS\x1b[0m",
        "\x1b[1mDOMAIN\x1b[0m"
    );
    println!("  {:<20} {:<8} {:<10} {}", "-------", "----", "------", "------");

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

        let server_label = format!("com.{}.server", config.project_name);
        let is_running = Command::new("launchctl")
            .args(["list", &server_label])
            .output()
            .is_ok_and(|o| o.status.success());

        let status = if is_running {
            "\x1b[32mrunning\x1b[0m"
        } else {
            "\x1b[31mstopped\x1b[0m"
        };

        println!(
            "  {:<20} {:<8} {:<20} {}",
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
