//! `bunker exec <project> <command…>`: any command as the app user, in the live release.
use crate::config::Config;
use crate::error::Result;
use crate::host::Host;
use crate::lima::vm::vm_name;
use crate::paths::ProjectDir;
use crate::run::Cmd;

/// The `cd` happens as `app`, after sudo: Lima's user can't be assumed to traverse `/srv/app`.
/// The command arrives as `sh`'s positional parameters, so `"$@"` keeps every argument as given
/// (limactl shell-quotes each one) and nothing in it is re-parsed by this shell.
const IN_RELEASE: &str = "cd /srv/app/current && exec \"$@\"";

pub fn exec_cmd(h: &Host, project: &str, args: &[String], tty: bool) -> Cmd {
    let mut c = Cmd::new(h.limactl()).arg("shell");
    if !tty {
        c = c.arg("--tty=false");
    }
    c.args(["--workdir", "/"])
        .arg(vm_name(project))
        .args(["sudo", "-u", "app", "-H", "sh", "-c", IN_RELEASE, "sh"])
        .args(args.iter().cloned())
        .interactive()
}

/// Runs the command attached to this terminal and returns its exit status.
pub fn exec(h: &Host, project: &str, args: &[String], tty: bool) -> Result<i32> {
    Config::load(&ProjectDir::under(&h.home, project))?;
    Ok(h.r.run(&exec_cmd(h, project, args, tty))?.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, host, project};
    use crate::run::{Output, RecordingRunner};

    #[test]
    fn exec_runs_as_app_in_the_live_release_with_args_verbatim() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let args: Vec<String> = ["composer", "show", "a b", "$HOME"]
            .map(String::from)
            .to_vec();
        let c = exec_cmd(&h, "t", &args, false);
        assert_eq!(
            c.args,
            [
                "shell",
                "--tty=false",
                "--workdir",
                "/",
                "bunker-t",
                "sudo",
                "-u",
                "app",
                "-H",
                "sh",
                "-c",
                "cd /srv/app/current && exec \"$@\"",
                "sh",
                "composer",
                "show",
                "a b",
                "$HOME"
            ]
        );
        assert!(c.interactive);
        assert!(
            !exec_cmd(&h, "t", &args, true)
                .args
                .contains(&"--tty=false".to_string())
        );
    }

    #[test]
    fn exec_passes_the_exit_code_through() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond("limactl", &["shell"], Output::fail(7, ""));
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        assert_eq!(exec(&h, "t", &["false".into()], false).unwrap(), 7);
    }
}
