//! `bunker artisan <project> -- <args…>`: artisan as the app user in the live release.
use crate::config::Config;
use crate::error::Result;
use crate::host::Host;
use crate::lima::vm::vm_name;
use crate::paths::ProjectDir;
use crate::run::Cmd;

/// The limactl call. Arguments go through as they are: limactl quotes each one for the guest's
/// shell (shellescape), so quoting them here would put literal quotes into artisan's argv.
pub fn artisan_cmd(h: &Host, project: &str, args: &[String], tty: bool) -> Cmd {
    let mut c = Cmd::new(h.limactl()).arg("shell");
    if !tty {
        c = c.arg("--tty=false");
    }
    c.args(["--workdir", "/"])
        .arg(vm_name(project))
        .args(["sudo", "-u", "app", "php", "/srv/app/current/artisan"])
        .args(args.iter().cloned())
        .interactive()
}

/// Runs artisan attached to this terminal and returns its exit status.
pub fn artisan(h: &Host, project: &str, args: &[String], tty: bool) -> Result<i32> {
    Config::load(&ProjectDir::under(&h.home, project))?;
    Ok(h.r.run(&artisan_cmd(h, project, args, tty))?.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, host, project};
    use crate::run::{Output, RecordingRunner};

    #[test]
    fn artisan_args_reach_limactl_verbatim() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let args = vec!["tinker".to_string(), "--execute=echo 'x'".to_string()];
        let c = artisan_cmd(&h, "t", &args, false);
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
                "php",
                "/srv/app/current/artisan",
                "tinker",
                "--execute=echo 'x'"
            ]
        );
        assert!(c.interactive);
        assert!(
            !artisan_cmd(&h, "t", &args, true)
                .args
                .contains(&"--tty=false".to_string())
        );
    }

    #[test]
    fn artisan_passes_the_exit_code_through() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond("limactl", &["shell"], Output::fail(3, ""));
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        assert_eq!(
            artisan(&h, "t", &["migrate:status".into()], false).unwrap(),
            3
        );
    }
}
