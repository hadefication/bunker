use bunker::cli::{Cli, Command};
use bunker::commands;
use bunker::error::Result;
use bunker::output::{self, ExitCode, Format, Report};
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
        Command::Init {
            project,
            path,
            domain,
            tunnel,
            php,
            force,
        } => finish(
            format,
            commands::init::run(commands::init::InitArgs {
                project,
                path,
                domain,
                tunnel,
                php,
                force,
            }),
        ),
    };
    std::process::exit(code as i32);
}

fn finish<R: Report>(format: Format, result: Result<R>) -> ExitCode {
    match result {
        Ok(r) => output::emit(format, &r),
        Err(e) => output::emit_error(format, &e),
    }
}
