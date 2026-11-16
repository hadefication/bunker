use bunker::cli::{Cli, Command, EnvCommand, NetCommand, ProxyCommand};
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
        Command::Up {
            project,
            overwrite_dns,
        } => {
            let store = bunker::secrets::keychain::KeychainStore::from_env();
            with_host(format, |h| {
                commands::up::up(h, &store, &project, overwrite_dns)
            })
        }
        Command::Start { project } => {
            with_host(format, |h| commands::lifecycle::start(h, &project))
        }
        Command::Stop { project } => with_host(format, |h| commands::lifecycle::stop(h, &project)),
        Command::Restart { project } => {
            with_host(format, |h| commands::lifecycle::restart(h, &project))
        }
        Command::Deploy {
            project,
            dirty,
            allow_advisories,
        } => {
            let store = bunker::secrets::keychain::KeychainStore::from_env();
            let opts = commands::deploy::DeployOpts {
                dirty,
                allow_advisories,
            };
            let progress = |s: &commands::deploy::StepResult| {
                if matches!(format, Format::Human) {
                    println!("{}", commands::deploy::step_line(s));
                }
            };
            with_host(format, |h| {
                commands::deploy::deploy(h, &store, &project, opts, &progress)
            })
        }
        Command::Rollback { project } => {
            with_host(format, |h| commands::rollback::rollback(h, &project))
        }
        Command::Destroy { project, yes } => match commands::destroy::confirm(&project, yes) {
            Ok(()) => with_host(format, |h| commands::destroy::destroy(h, &project)),
            Err(e) => output::emit_error(format, &e),
        },
        Command::Env { cmd } => {
            let args = match cmd {
                EnvCommand::Set {
                    key,
                    rest,
                    stdin,
                    plain,
                    bind,
                } => commands::env::EnvArgs::Set {
                    key,
                    rest,
                    stdin,
                    plain,
                    bind,
                },
                EnvCommand::Unset { key, project } => {
                    commands::env::EnvArgs::Unset { key, project }
                }
                EnvCommand::List { project } => commands::env::EnvArgs::List { project },
            };
            let read_stdin = || {
                let mut v = Vec::new();
                std::io::Read::read_to_end(&mut std::io::stdin(), &mut v).map(|_| v)
            };
            // SAFETY: isatty only inspects the descriptor.
            let tty = unsafe { libc::isatty(0) } == 1;
            match commands::env::prepare(args, &read_stdin, tty) {
                Ok(p) => {
                    let store = bunker::secrets::keychain::KeychainStore::from_env();
                    with_host(format, |h| commands::env::run(h, &store, p))
                }
                Err(e) => output::emit_error(format, &e),
            }
        }
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
