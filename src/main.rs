mod cli;
mod commands;
mod config;
mod framework;
mod templates;

use clap::{CommandFactory, Parser};
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
        Command::Info { project, verbose } => commands::info::run(project, verbose),
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
        Command::Update { project } => commands::update::run(project),
        Command::SelfUpdate => commands::self_update::run(),
        Command::Completions { shell } => {
            let shell = shell
                .parse::<clap_complete::Shell>()
                .map_err(|_| anyhow::anyhow!("Unknown shell '{}'. Use: bash, zsh, fish", shell));
            match shell {
                Ok(s) => {
                    clap_complete::generate(
                        s,
                        &mut Cli::command(),
                        "bunker",
                        &mut std::io::stdout(),
                    );
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
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
