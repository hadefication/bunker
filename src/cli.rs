//! Command-line definitions.
use clap::{Parser, Subcommand};

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
    /// Print the version, commit and build date.
    Version,
}
