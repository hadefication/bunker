//! `bunker shell <project>`: an interactive shell in the guest, as Lima's user (who has sudo).
use crate::config::Config;
use crate::error::{BunkerError, Result};
use crate::host::Host;
use crate::lima::vm::vm_name;
use crate::output::ExitCode;
use crate::paths::ProjectDir;
use crate::run::Cmd;

pub fn shell(h: &Host, project: &str, tty: bool) -> Result<i32> {
    Config::load(&ProjectDir::under(&h.home, project))?;
    if !tty {
        return Err(BunkerError::new(
            "needs_tty",
            ExitCode::Refused,
            "bunker shell is interactive and needs a terminal",
        )
        .hint(format!(
            "for one command, use bunker exec {project} <command…>, or run bunker shell in a terminal"
        )));
    }
    let c = Cmd::new(h.limactl())
        .args(["shell", "--workdir", "/"])
        .arg(vm_name(project))
        .interactive();
    Ok(h.r.run(&c)?.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, host, project};
    use crate::run::RecordingRunner;

    #[test]
    fn shell_refuses_without_tty() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let e = shell(&h, "t", false).unwrap_err();
        assert_eq!(
            (e.code, e.exit),
            ("needs_tty", crate::output::ExitCode::Refused)
        );
        assert!(r.calls().is_empty());
    }

    #[test]
    fn shell_opens_an_interactive_lima_shell() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        shell(&h, "t", true).unwrap();
        let c = &r.calls()[0];
        assert_eq!(c.args, ["shell", "--workdir", "/", "bunker-t"]);
        assert!(c.interactive);
    }
}
