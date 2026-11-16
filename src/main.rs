use bunker::cli::{Cli, Command};
use bunker::commands;
use bunker::output::{self, ExitCode, Format};
use clap::Parser;

fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            // --help and --version are clap "errors" that print to stdout and exit 0.
            let usage = e.use_stderr();
            let _ = e.print();
            std::process::exit(if usage { ExitCode::Usage as i32 } else { 0 });
        }
    };
    let format = if cli.json {
        Format::Json
    } else {
        Format::Human
    };
    let code = match cli.command {
        Command::Version => output::emit(format, &commands::version::VersionReport),
    };
    std::process::exit(code as i32);
}
