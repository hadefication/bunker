use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "bunker",
    about = "Local production deployment for web apps on macOS",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Set up a new project (run from project directory)
    Init,

    /// Start all services
    Start {
        /// Project name (defaults to current directory)
        project: Option<String>,
    },

    /// Stop all services
    Stop {
        /// Project name (defaults to current directory)
        project: Option<String>,
    },

    /// Restart all services
    Restart {
        /// Project name (defaults to current directory)
        project: Option<String>,
    },

    /// Show service status
    Status {
        /// Project name (defaults to current directory)
        project: Option<String>,
    },

    /// Run in foreground (Ctrl+C to stop)
    Run {
        /// Project name (defaults to current directory)
        project: Option<String>,
    },

    /// View logs
    Logs {
        /// Project name (defaults to current directory)
        project: Option<String>,

        /// Filter by service: server, tunnel, queue, scheduler, access
        #[arg(long)]
        service: Option<String>,

        /// Follow log output
        #[arg(long, short)]
        follow: bool,
    },

    /// List all bunkered projects
    List,

    /// Remove a project's bunker config
    Teardown {
        /// Project name (defaults to current directory)
        project: Option<String>,
    },

    /// Open project config in $EDITOR
    Edit {
        /// Project name (defaults to current directory)
        project: Option<String>,
    },
}
