//! Every external program bunker runs goes through a `Runner`, so tests can record the exact
//! invocations and prove that no secret value ever reaches a command line.
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    /// Bytes fed to the program's stdin. The only channel a secret value may travel on.
    pub stdin: Option<Vec<u8>>,
    pub env: Vec<(String, String)>,
    /// Inherit the terminal (stdin, stdout, stderr) instead of capturing.
    pub interactive: bool,
}

impl Cmd {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    pub fn interactive(mut self) -> Self {
        self.interactive = true;
        self
    }

    /// `program arg arg`, for error messages. Never call this on a command that could carry a secret in argv (none may).
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn ok(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 0,
            stdout: stdout.into(),
            stderr: vec![],
        }
    }

    pub fn fail(status: i32, stderr: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            stdout: vec![],
            stderr: stderr.into(),
        }
    }

    pub fn success(&self) -> bool {
        self.status == 0
    }

    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

pub trait Runner: Send + Sync {
    /// Runs the command. A non-zero exit is an `Ok(Output)`; only failing to start it is an error.
    fn run(&self, cmd: &Cmd) -> Result<Output>;

    /// Runs the command, calling `on_line` for each stdout line as it arrives (`logs -f`), and
    /// returns the exit status. The default runs it to the end first.
    fn stream(&self, cmd: &Cmd, on_line: &mut dyn FnMut(&str)) -> Result<i32> {
        let out = self.run(cmd)?;
        for l in out.stdout_str().lines() {
            on_line(l);
        }
        Ok(out.status)
    }

    /// Runs the command and turns a non-zero exit into an error carrying its stderr.
    fn run_ok(&self, cmd: &Cmd) -> Result<Output> {
        let out = self.run(cmd)?;
        if out.success() {
            Ok(out)
        } else {
            Err(BunkerError::new(
                "command_failed",
                ExitCode::Failure,
                format!(
                    "{} exited {}: {}",
                    cmd.program,
                    out.status,
                    out.stderr_str().trim()
                ),
            ))
        }
    }
}

pub struct SystemRunner;

impl Runner for SystemRunner {
    fn stream(&self, cmd: &Cmd, on_line: &mut dyn FnMut(&str)) -> Result<i32> {
        use std::io::BufRead;
        let mut c = Command::new(&cmd.program);
        c.args(&cmd.args)
            .envs(cmd.env.iter().map(|(k, v)| (k, v)))
            .stdin(if cmd.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = c
            .spawn()
            .map_err(|e| BunkerError::io(format!("cannot run {}", cmd.program), e))?;
        if let Some(bytes) = &cmd.stdin {
            let mut pipe = child.stdin.take().expect("stdin was piped");
            let bytes = bytes.clone();
            std::thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            });
        }
        let stdout = child.stdout.take().expect("stdout was piped");
        for line in std::io::BufReader::new(stdout).lines() {
            match line {
                Ok(l) => on_line(&l),
                Err(_) => break,
            }
        }
        let status = child
            .wait()
            .map_err(|e| BunkerError::io(format!("cannot wait for {}", cmd.program), e))?;
        Ok(status.code().unwrap_or(1))
    }

    fn run(&self, cmd: &Cmd) -> Result<Output> {
        let mut c = Command::new(&cmd.program);
        c.args(&cmd.args).envs(cmd.env.iter().map(|(k, v)| (k, v)));
        if cmd.interactive {
            let status = c
                .status()
                .map_err(|e| BunkerError::io(format!("cannot run {}", cmd.program), e))?;
            return Ok(Output {
                status: status.code().unwrap_or(1),
                ..Output::default()
            });
        }
        c.stdin(if cmd.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        let mut child = c
            .spawn()
            .map_err(|e| BunkerError::io(format!("cannot run {}", cmd.program), e))?;
        if let Some(bytes) = &cmd.stdin {
            let mut pipe = child.stdin.take().expect("stdin was piped");
            let bytes = bytes.clone();
            // A writer thread, so a program that fills stdout before reading stdin cannot deadlock us.
            std::thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            });
        }
        let out = child
            .wait_with_output()
            .map_err(|e| BunkerError::io(format!("cannot wait for {}", cmd.program), e))?;
        Ok(Output {
            status: out.status.code().unwrap_or(1),
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }
}

type Responder = Box<dyn Fn(&Cmd) -> Output + Send + Sync>;

struct Expectation {
    program: String,
    prefix: Vec<String>,
    respond: Responder,
}

/// Test double: answers from queued responses (first unused match on program plus an args
/// prefix, else success with empty output) and records every command it was asked to run.
#[derive(Default)]
pub struct RecordingRunner {
    expectations: Mutex<Vec<Expectation>>,
    calls: Mutex<Vec<Cmd>>,
}

impl RecordingRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn respond(&self, program: &str, prefix: &[&str], out: Output) -> &Self {
        self.respond_with(program, prefix, move |_| out.clone())
    }

    pub fn respond_with(
        &self,
        program: &str,
        prefix: &[&str],
        f: impl Fn(&Cmd) -> Output + Send + Sync + 'static,
    ) -> &Self {
        self.expectations.lock().unwrap().push(Expectation {
            program: program.to_string(),
            prefix: prefix.iter().map(|s| s.to_string()).collect(),
            respond: Box::new(f),
        });
        self
    }

    pub fn calls(&self) -> Vec<Cmd> {
        self.calls.lock().unwrap().clone()
    }

    /// Panics if any recorded program name or argument contains one of `secrets`.
    pub fn assert_no_secret_in_argv(&self, secrets: &[&str]) {
        for cmd in self.calls() {
            for s in secrets.iter().filter(|s| !s.is_empty()) {
                let hit = cmd.program.contains(s) || cmd.args.iter().any(|a| a.contains(s));
                assert!(
                    !hit,
                    "a secret value reached the command line of {}",
                    cmd.program
                );
            }
        }
    }
}

impl Runner for RecordingRunner {
    fn run(&self, cmd: &Cmd) -> Result<Output> {
        self.calls.lock().unwrap().push(cmd.clone());
        let mut exp = self.expectations.lock().unwrap();
        let hit = exp
            .iter()
            .position(|e| e.program == cmd.program && cmd.args.starts_with(&e.prefix));
        Ok(match hit {
            Some(i) => {
                let e = exp.remove(i);
                (e.respond)(cmd)
            }
            None => Output::ok(Vec::new()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "secret")]
    fn recording_runner_flags_secret_in_argv() {
        let r = RecordingRunner::new();
        r.run(&Cmd::new("mysql").arg("-ps3cr3t-fixture")).unwrap();
        r.assert_no_secret_in_argv(&["s3cr3t-fixture"]);
    }

    #[test]
    fn recording_runner_allows_secret_on_stdin() {
        let r = RecordingRunner::new();
        r.run(
            &Cmd::new("limactl")
                .args(["shell", "vm", "sudo", "tee"])
                .stdin("KEY=s3cr3t-fixture"),
        )
        .unwrap();
        r.assert_no_secret_in_argv(&["s3cr3t-fixture"]);
    }

    #[test]
    fn recording_runner_answers_first_matching_prefix_once() {
        let r = RecordingRunner::new();
        r.respond("git", &["status"], Output::ok("M x"));
        assert_eq!(
            r.run(&Cmd::new("git").args(["status", "--porcelain"]))
                .unwrap()
                .stdout_str(),
            "M x"
        );
        assert_eq!(
            r.run(&Cmd::new("git").args(["status", "--porcelain"]))
                .unwrap()
                .stdout_str(),
            ""
        );
        assert_eq!(r.calls().len(), 2);
    }

    #[test]
    fn system_runner_feeds_stdin_and_captures() {
        let out = SystemRunner
            .run(&Cmd::new("/bin/cat").stdin("hello"))
            .unwrap();
        assert_eq!((out.status, out.stdout_str()), (0, "hello".to_string()));
    }
}
