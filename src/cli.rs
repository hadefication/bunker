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
    /// Print the version, commit and build date.
    Version,
}
