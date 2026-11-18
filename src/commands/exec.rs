//! `bunker exec [project] <command…>`: any command as the app user, in the live release.
use crate::config::validate::validate_project_name;
use crate::config::{Config, resolve_project_in};
use crate::error::{BunkerError, Result};
use crate::host::Host;
use crate::lima::vm::vm_name;
use crate::output::ExitCode;
use crate::paths::ProjectDir;
use crate::run::Cmd;
use std::path::Path;

/// The `cd` happens as `app`, after sudo: Lima's user can't be assumed to traverse `/srv/app`.
/// The command arrives as `sh`'s positional parameters, so `"$@"` keeps every argument as given
/// (limactl shell-quotes each one) and nothing in it is re-parsed by this shell.
const IN_RELEASE: &str = "cd /srv/app/current && exec \"$@\"";

/// Splits `[project] [--] <command…>`. The first word is the project only when a project by that
/// name exists; otherwise the project is the one whose repo holds `cwd`. (clap has already eaten a
/// `--` straight after `exec`, so a command named like a project needs the project named first.)
pub fn target(home: &Path, cwd: &Path, args: &[String]) -> Result<(ProjectDir, Vec<String>)> {
    let names_project =
        |w: &str| validate_project_name(w).is_ok() && ProjectDir::under(home, w).config().is_file();
    let (named, rest) = match args {
        [first, rest @ ..] if names_project(first) => match rest {
            [dd, rest @ ..] if dd == "--" => (Some(first.as_str()), rest),
            _ => (Some(first.as_str()), rest),
        },
        _ => (None, args),
    };
    if rest.is_empty() {
        return Err(BunkerError::new(
            "usage",
            ExitCode::Usage,
            "bunker exec [project] <command…>: no command given",
        ));
    }
    let pd = resolve_project_in(home, named, cwd, &mut std::io::stderr())?;
    Ok((pd, rest.to_vec()))
}

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

    fn strs(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn target_takes_a_named_project_with_or_without_a_double_dash() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let root = std::path::Path::new("/");
        let (p, c) = target(&h.home, root, &strs(&["t", "php", "-v"])).unwrap();
        assert_eq!((p.name.as_str(), c), ("t", strs(&["php", "-v"])));
        let (p, c) = target(&h.home, root, &strs(&["t", "--", "php", "-v"])).unwrap();
        assert_eq!((p.name.as_str(), c), ("t", strs(&["php", "-v"])));
    }

    #[test]
    fn target_falls_back_to_the_project_holding_the_current_directory() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t"); // its repo is h.home
        let cmd = strs(&["php", "artisan", "app:info"]);
        let (p, c) = target(&h.home, &h.home, &cmd).unwrap();
        assert_eq!((p.name.as_str(), c), ("t", cmd));
        // A command named like a project runs when the project is named first.
        let (p, c) = target(&h.home, &h.home, &strs(&["t", "--", "t"])).unwrap();
        assert_eq!((p.name.as_str(), c), ("t", strs(&["t"])));
    }

    #[test]
    fn target_refuses_a_project_without_a_command_and_a_cwd_outside_projects() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let root = std::path::Path::new("/");
        let e = target(&h.home, root, &strs(&["t"])).unwrap_err();
        assert_eq!(e.code, "usage");
        let e = target(&h.home, root, &strs(&["t", "--"])).unwrap_err();
        assert_eq!(e.code, "usage");
        let e = target(&h.home, root, &strs(&["nope", "-v"])).unwrap_err();
        assert_eq!(e.code, "project_not_found");
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
