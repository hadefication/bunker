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
    Init {
        /// Project name (defaults to directory basename)
        #[arg(long)]
        name: Option<String>,

        /// Port to bind (defaults to next available from 8700)
        #[arg(long)]
        port: Option<u16>,

        /// Domain for public access (defaults to free cfargotunnel.com URL)
        #[arg(long)]
        domain: Option<String>,

        /// Tunnel name (defaults to project name)
        #[arg(long)]
        tunnel: Option<String>,

        /// Enable Laravel scheduler (schedule:work)
        #[arg(long)]
        scheduler: bool,

        /// Path to PHP binary
        #[arg(long)]
        php: Option<String>,

        /// Path to FrankenPHP binary
        #[arg(long)]
        frankenphp: Option<String>,

        /// Path to cloudflared binary
        #[arg(long)]
        cloudflared: Option<String>,

        /// Accept all defaults, no prompts (implies --no-scheduler unless --scheduler is set)
        #[arg(long, short = 'y')]
        yes: bool,

        /// Preview what would be generated without creating anything
        #[arg(long)]
        dry_run: bool,
    },

    /// Remove a project's bunker config
    Teardown {
        /// Project name (defaults to current directory)
        project: Option<String>,

        /// Skip confirmation prompts
        #[arg(long, short = 'y')]
        yes: bool,
    },

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

    /// Show app details
    Info {
        /// Project name (defaults to current directory)
        project: Option<String>,

        /// Show full details (tunnel UUID, binary paths)
        #[arg(long, short)]
        verbose: bool,
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

    /// Open project config in $EDITOR
    Edit {
        /// Project name (defaults to current directory)
        project: Option<String>,
    },

    /// Re-generate configs from bunker.conf (Caddyfile, plists, cloudflared.yml)
    Update {
        /// Project name (defaults to current directory)
        project: Option<String>,
    },

    /// Update bunker to the latest release
    SelfUpdate,

    /// Generate shell completions
    Completions {
        /// Shell to generate for: bash, zsh, fish
        shell: String,
    },
}
