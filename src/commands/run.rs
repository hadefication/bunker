use std::process::Command;

use crate::config::{ProjectConfig, resolve_project};
use crate::framework::laravel::Laravel;
use crate::framework::{Framework, FrameworkKind};
use crate::output;

pub fn run(project: Option<String>) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;

    // Check for npx
    if which::which("npx").is_err() {
        anyhow::bail!("npx not found. Install Node.js for foreground mode.");
    }

    output::info(&format!(
        "Running {} in foreground (Ctrl+C to stop)...",
        name
    ));
    println!();

    let mut commands = vec![
        format!(
            "\"{}\" run --config \"{}\"",
            config.frankenphp_path,
            config.project_dir().join("Caddyfile").display()
        ),
        format!(
            "\"{}\" tunnel --no-autoupdate --config \"{}\" run",
            config.cloudflared_path,
            config.project_dir().join("cloudflared.yml").display()
        ),
    ];

    let mut names = vec!["server".to_string(), "tunnel".to_string()];
    let mut colors = vec!["blue".to_string(), "yellow".to_string()];

    // Add framework-specific services
    let framework_services = match config.framework {
        FrameworkKind::Laravel => {
            let laravel = Laravel {
                php_path: config.php_path.clone(),
                project_path: config.project_path.clone(),
                scheduler_enabled: config.scheduler_enabled,
            };
            laravel.extra_services()
        }
    };

    let svc_colors = ["green", "magenta", "cyan"];
    for (i, svc) in framework_services.iter().enumerate() {
        let cmd = svc
            .command
            .iter()
            .map(|a| format!("\"{}\"", a))
            .collect::<Vec<_>>()
            .join(" ");
        commands.push(cmd);
        names.push(svc.name.clone());
        colors.push(svc_colors.get(i).unwrap_or(&"white").to_string());
    }

    let status = Command::new("npx")
        .args([
            "concurrently",
            "--names",
            &names.join(","),
            "--prefix-colors",
            &colors.join(","),
            "--kill-others",
        ])
        .args(&commands)
        .current_dir(&config.project_path)
        .env("APP_ENV", "production")
        .status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}
