use std::process::Command;

use crate::config::{ProjectConfig, resolve_project};
use crate::output;

pub fn run(project: Option<String>, service: Option<String>, follow: bool) -> anyhow::Result<()> {
    let name = resolve_project(project)?;
    let config = ProjectConfig::load(&name)?;
    let logs_dir = config.logs_dir();

    if !logs_dir.exists() {
        anyhow::bail!("No logs directory found for {}", name);
    }

    let log_files: Vec<String> = if let Some(svc) = &service {
        match svc.as_str() {
            "server" => vec![
                "frankenphp-stdout.log".to_string(),
                "frankenphp-stderr.log".to_string(),
            ],
            "tunnel" => vec![
                "cloudflared-stdout.log".to_string(),
                "cloudflared-stderr.log".to_string(),
            ],
            "queue" => vec![
                "queue-stdout.log".to_string(),
                "queue-stderr.log".to_string(),
            ],
            "scheduler" => vec![
                "scheduler-stdout.log".to_string(),
                "scheduler-stderr.log".to_string(),
            ],
            "access" => vec!["caddy-access.log".to_string()],
            _ => anyhow::bail!(
                "Unknown service: {}. Use: server, tunnel, queue, scheduler, access",
                svc
            ),
        }
    } else {
        // All logs
        std::fs::read_dir(&logs_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect()
    };

    // Filter to existing files
    let existing: Vec<String> = log_files
        .into_iter()
        .filter(|f| logs_dir.join(f).exists())
        .map(|f| logs_dir.join(f).display().to_string())
        .collect();

    if existing.is_empty() {
        output::warn("No log files found.");
        return Ok(());
    }

    let mut args = vec![if follow {
        "-f".to_string()
    } else {
        "-n".to_string()
    }];

    if !follow {
        args.push("50".to_string());
    }

    args.extend(existing);

    let status = Command::new("tail").args(&args).status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}
