use bunker::cli::{Cli, Command, NetCommand, ProxyCommand};
use bunker::commands;
use bunker::commands::net::NetArgs;
use bunker::error::Result;
use bunker::host::Host;
use bunker::lima::confine::SystemProbe;
use bunker::output::{self, ExitCode, Format, Report};
use bunker::run::SystemRunner;
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
        Command::Up { project } => with_host(format, |h| commands::up::up(h, &project)),
        Command::Start { project } => {
            with_host(format, |h| commands::lifecycle::start(h, &project))
        }
        Command::Stop { project } => with_host(format, |h| commands::lifecycle::stop(h, &project)),
        Command::Restart { project } => {
            with_host(format, |h| commands::lifecycle::restart(h, &project))
        }
        Command::Destroy { project, yes } => match commands::destroy::confirm(&project, yes) {
            Ok(()) => with_host(format, |h| commands::destroy::destroy(h, &project)),
            Err(e) => output::emit_error(format, &e),
        },
        Command::Proxy {
            cmd: ProxyCommand::Serve,
        } => finish(format, commands::proxy::serve()),
        Command::Net { cmd } => finish(
            format,
            commands::net::run(match cmd {
                NetCommand::Allow {
                    host,
                    project,
                    build,
                } => NetArgs::Allow {
                    host,
                    project,
                    build,
                },
                NetCommand::Deny { host, project } => NetArgs::Deny { host, project },
                NetCommand::Rm { host, project } => NetArgs::Rm { host, project },
                NetCommand::Reload { project } => NetArgs::Reload { project },
                NetCommand::Log {
                    project,
                    blocked,
                    n,
                } => NetArgs::Log {
                    project,
                    blocked,
                    n,
                },
            }),
        ),
    };
    std::process::exit(code as i32);
}

/// Runs a command that needs the machine: the system runner, the Seatbelt probe, real paths.
fn with_host<R: Report>(format: Format, f: impl FnOnce(&Host) -> Result<R>) -> ExitCode {
    let runner = SystemRunner;
    let probe = SystemProbe {
        lima_home: bunker::host::lima_home(),
    };
    match Host::system(&runner, &probe) {
        Ok(h) => finish(format, f(&h)),
        Err(e) => output::emit_error(format, &e),
    }
}

fn finish<R: Report>(format: Format, result: Result<R>) -> ExitCode {
    match result {
        Ok(r) => output::emit(format, &r),
        Err(e) => output::emit_error(format, &e),
    }
}
