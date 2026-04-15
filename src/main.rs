mod cli;
mod commands;
mod config;
mod framework;
mod templates;

use clap::Parser;
use cli::{Cli, Command};

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Init {
            name,
            port,
            domain,
            tunnel,
            scheduler,
            php,
            frankenphp,
            cloudflared,
            yes,
            dry_run,
        } => commands::init::run(commands::init::InitArgs {
            name,
            port,
            domain,
            tunnel,
            scheduler,
            php,
            frankenphp,
            cloudflared,
            yes,
            dry_run,
        }),
        Command::Start { project } => commands::lifecycle::start(project),
        Command::Stop { project } => commands::lifecycle::stop(project),
        Command::Restart { project } => commands::lifecycle::restart(project),
        Command::Status { project } => commands::lifecycle::status(project),
        Command::Run { project } => commands::run::run(project),
        Command::Logs {
            project,
            service,
            follow,
        } => commands::logs::run(project, service, follow),
        Command::List => commands::list::run(),
        Command::Teardown { project, yes } => commands::teardown::run(project, yes),
        Command::Edit { project } => commands::edit::run(project),
    };

    if let Err(e) = result {
        output::error(&e.to_string());
        std::process::exit(1);
    }
}

pub mod output {
    use colored::Colorize;

    pub fn info(msg: &str) {
        println!("{} {}", "==>".blue(), msg);
    }

    pub fn success(msg: &str) {
        println!("{} {}", "==>".green(), msg);
    }

    pub fn warn(msg: &str) {
        println!("{} {}", "==>".yellow(), msg);
    }

    pub fn error(msg: &str) {
        eprintln!("{} {}", "==>".red(), msg);
    }
}
