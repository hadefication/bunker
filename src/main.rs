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
        Command::TunnelRun { project } => std::process::exit(with_host_code(format, |h| {
            let cmd = commands::up::tunnel_gate(h, &project)?;
            use std::os::unix::process::CommandExt;
            // exec only returns on failure; launchd then retries after the agent's throttle.
            let err = std::process::Command::new(&cmd.program)
                .args(&cmd.args)
                .exec();
            Err(bunker::error::BunkerError::io(
                format!("cannot start {}", cmd.program),
                err,
            ))
        })),
        Command::Status { project } => {
            with_host(format, |h| commands::status::status(h, project.as_deref()))
        }
        Command::Logs { args, follow, n } => run_logs(format, &args, follow, n),
        Command::Artisan { project, args } => std::process::exit(with_host_code(format, |h| {
            commands::artisan::artisan(h, &project, &args, stdin_tty())
        })),
        Command::Exec { project, command } => std::process::exit(with_host_code(format, |h| {
            commands::exec::exec(h, &project, &command, stdin_tty())
        })),
        Command::Shell { project } => std::process::exit(with_host_code(format, |h| {
            commands::shell::shell(h, &project, stdin_tty())
        })),
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

fn stdin_tty() -> bool {
    // SAFETY: isatty only inspects the descriptor.
    unsafe { libc::isatty(0) == 1 }
}

/// For a command whose exit status is the guest program's (artisan, shell).
fn with_host_code(format: Format, f: impl FnOnce(&Host) -> Result<i32>) -> i32 {
    let runner = SystemRunner;
    let probe = SystemProbe {
        lima_home: bunker::host::lima_home(),
    };
    match Host::system(&runner, &probe).and_then(|h| f(&h)) {
        Ok(code) => code,
        Err(e) => output::emit_error(format, &e) as i32,
    }
}

/// Lines stream as they arrive; `--json` gathers them (or, with `-f`, prints one object per line).
fn run_logs(format: Format, args: &[String], follow: bool, n: usize) -> ExitCode {
    let home = bunker::paths::bunker_home();
    let (project, source) = match commands::logs::parse_args(&home, args) {
        Ok(x) => x,
        Err(e) => return output::emit_error(format, &e),
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let pd = match bunker::config::resolve_project(project.as_deref(), &cwd) {
        Ok(pd) => pd,
        Err(e) => return output::emit_error(format, &e),
    };
    let store = bunker::secrets::keychain::KeychainStore::from_env();
    let gather = matches!(format, Format::Json) && !follow;
    with_host(format, |h| {
        let mut lines = Vec::new();
        commands::logs::logs(h, &store, &pd.name, source, follow, n, &mut |l| {
            if gather {
                lines.push(l.to_string());
            } else if matches!(format, Format::Json) {
                println!("{}", serde_json::json!({"line": l}));
            } else {
                println!("{l}");
            }
        })?;
        Ok(commands::logs::LinesReport { lines })
    })
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
