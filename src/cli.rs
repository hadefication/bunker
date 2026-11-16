//! Command-line definitions.
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// `2.0.0-alpha.0 (<sha>, <date>)`; clap prefixes the binary name for `--version` and `-V`.
pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("BUNKER_COMMIT"),
    ", ",
    env!("BUNKER_BUILD_DATE"),
    ")"
);

#[derive(Debug, Parser)]
#[command(
    name = "bunker",
    version = VERSION,
    about = "Host personal web apps on this Mac in confined Lima VMs behind Cloudflare tunnels",
    after_help = "Every command prints readable output by default. For agents and scripts: add --json."
)]
pub struct Cli {
    /// Print machine-readable JSON instead of human output.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Detect the app at --path and write ~/.bunker/<project>/bunker.toml.
    Init {
        /// Project name: lowercase letters, digits and hyphens.
        project: String,
        /// The app's repository on this Mac.
        #[arg(long)]
        path: PathBuf,
        /// Public hostname, served through a Cloudflare named tunnel.
        #[arg(long)]
        domain: Option<String>,
        /// Tunnel name (defaults to the project name when --domain is given).
        #[arg(long)]
        tunnel: Option<String>,
        /// PHP version, instead of reading composer.json's constraint.
        #[arg(long)]
        php: Option<String>,
        /// Overwrite an existing bunker.toml.
        #[arg(long)]
        force: bool,
    },
    /// Edit and inspect the project's outbound allowlist.
    Net {
        #[command(subcommand)]
        cmd: NetCommand,
    },
    /// Run the egress proxy (started by launchd as com.bunker.proxy).
    #[command(hide = true)]
    Proxy {
        #[command(subcommand)]
        cmd: ProxyCommand,
    },
    /// Print the version, commit and build date.
    Version,
}

#[derive(Debug, Subcommand)]
pub enum ProxyCommand {
    /// Serve every project's proxy port until SIGTERM.
    Serve,
}

#[derive(Debug, Subcommand)]
pub enum NetCommand {
    /// Let the running site reach HOST (or, with --build, only during deploys). Removes it from deny.
    Allow {
        /// host, *.domain or public IP, with an optional :port (80 and 443 otherwise).
        host: String,
        /// Project name; defaults to the project whose repo holds the current directory.
        project: Option<String>,
        /// Add to [network].build instead: allowed only while a deploy runs.
        #[arg(long)]
        build: bool,
    },
    /// Block HOST even if a wildcard allows it. Removes it from allow and build.
    Deny {
        host: String,
        project: Option<String>,
    },
    /// Remove HOST from allow, build and deny.
    Rm {
        host: String,
        project: Option<String>,
    },
    /// Validate a hand edit of bunker.toml, print what changed, and log it.
    Reload { project: Option<String> },
    /// Show recent proxy decisions, policy changes and leases.
    Log {
        project: Option<String>,
        /// Only blocked requests.
        #[arg(long)]
        blocked: bool,
        /// How many events.
        #[arg(short = 'n', default_value_t = 50)]
        n: usize,
    },
}
