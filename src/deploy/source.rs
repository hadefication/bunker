//! The release's source: a tar of the repo's tracked files, never any `.env*` file.
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::SystemTime;

/// The tar, built in memory (a release is source only: no vendor, no node_modules).
pub struct Archive {
    pub tar: Vec<u8>,
    pub sha: String,
    pub dirty: bool,
}

/// True for a path with any component starting with `.env` (`.env`, `.env.production`,
/// `config/.env.example`, `.envs/x`): secrets reach the guest only through bunker env.
fn is_env_path(p: &str) -> bool {
    p.split('/').any(|c| c.starts_with(".env"))
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| BunkerError::io("cannot run git", e))?;
    if !out.status.success() {
        return Err(BunkerError::new(
            "git_failed",
            ExitCode::Failure,
            format!(
                "git {} failed in {}: {}",
                args.join(" "),
                repo.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    Ok(out.stdout)
}

/// Clean: `git archive HEAD`. Dirty: the tracked files as they are in the working tree. Either way
/// only tracked files, minus every `.env*` path; a tracked modification needs `dirty`.
pub fn build_archive(repo: &Path, dirty: bool) -> Result<Archive> {
    let sha = String::from_utf8_lossy(&git(repo, &["rev-parse", "HEAD"])?)
        .trim()
        .to_string();
    let modified = !git(repo, &["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    if modified && !dirty {
        return Err(BunkerError::new(
            "dirty_tree",
            ExitCode::Refused,
            format!(
                "{} has uncommitted changes to tracked files",
                repo.display()
            ),
        )
        .hint("commit them, or deploy the working tree as it is with --dirty"));
    }
    let tar = if modified {
        let list = git(repo, &["ls-files", "-z"])?;
        let files: Vec<&str> = list
            .split(|b| *b == 0)
            .filter_map(|f| std::str::from_utf8(f).ok())
            .filter(|f| !f.is_empty() && !is_env_path(f))
            .collect();
        tar_worktree(repo, &files)?
    } else {
        // Every path component starting with .env is excluded: the file itself, or a directory.
        git(
            repo,
            &[
                "archive",
                "--format=tar",
                "HEAD",
                "--",
                ".",
                ":(exclude,glob)**/.env*",
                ":(exclude,glob)**/.env*/**",
            ],
        )?
    };
    Ok(Archive {
        tar,
        sha,
        dirty: modified,
    })
}

/// The working-tree files, by name from stdin, without macOS metadata or ownership.
fn tar_worktree(repo: &Path, files: &[&str]) -> Result<Vec<u8>> {
    let list: Vec<u8> = files.iter().flat_map(|f| f.bytes().chain([0])).collect();
    run_with_stdin(
        Command::new("/usr/bin/tar")
            .current_dir(repo)
            .env("COPYFILE_DISABLE", "1")
            .args([
                "--no-mac-metadata",
                "--no-xattrs",
                "--no-acls",
                "--no-fflags",
                "--uid",
                "0",
                "--gid",
                "0",
                "--null",
                "-cf",
                "-",
                "-T",
                "-",
            ]),
        &list,
        "tar",
    )
}

fn run_with_stdin(cmd: &mut Command, input: &[u8], what: &str) -> Result<Vec<u8>> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BunkerError::io(format!("cannot run {what}"), e))?;
    let mut stdin = child.stdin.take().expect("piped");
    let input = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let out = child
        .wait_with_output()
        .map_err(|e| BunkerError::io(format!("{what} failed"), e))?;
    let _ = writer.join();
    if !out.status.success() {
        return Err(BunkerError::new(
            "archive_failed",
            ExitCode::Failure,
            format!(
                "{what} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    Ok(out.stdout)
}

/// `20261116T030405Z-abc1234`, `-dirty` when the working tree went out.
pub fn release_name(now: SystemTime, sha: &str, dirty: bool) -> String {
    let short = &sha[..sha.len().min(7)];
    let suffix = if dirty { "-dirty" } else { "" };
    format!("{}-{short}{suffix}", crate::time::compact(now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        git(p, &["init", "-q"]);
        for f in [
            ".env",
            ".env.production",
            "config/.env.example",
            ".envs/x",
            "app/x.php",
        ] {
            let path = p.join(f);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("{f}\n")).unwrap();
        }
        git(p, &["add", "-f", "."]);
        git(p, &["commit", "-qm", "init"]);
        d
    }

    fn names(tar: &[u8]) -> Vec<String> {
        use std::io::Write;
        let mut c = Command::new("/usr/bin/tar")
            .args(["-tf", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(tar).unwrap();
        let out = c.wait_with_output().unwrap();
        assert!(out.status.success());
        let mut v: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .filter(|l| !l.ends_with('/'))
            .map(str::to_string)
            .collect();
        v.sort();
        v
    }

    #[test]
    fn archive_drops_env_files() {
        let r = repo();
        let a = build_archive(r.path(), false).unwrap();
        assert_eq!(names(&a.tar), ["app/x.php"]);
        assert!(!a.dirty);
        assert_eq!(a.sha.len(), 40);
        // The dirty path filters the same way.
        let a = build_archive(r.path(), true).unwrap();
        assert_eq!(names(&a.tar), ["app/x.php"]);
    }

    #[test]
    fn dirty_tree_refused_without_flag() {
        let r = repo();
        std::fs::write(r.path().join("app/x.php"), "changed\n").unwrap();
        let e = build_archive(r.path(), false).err().unwrap();
        assert_eq!(
            (e.code, e.exit),
            ("dirty_tree", crate::output::ExitCode::Refused)
        );
    }

    #[test]
    fn dirty_flag_ships_tracked_worktree_not_untracked() {
        let r = repo();
        std::fs::write(r.path().join("app/x.php"), "changed\n").unwrap();
        std::fs::write(r.path().join("app/untracked.php"), "u\n").unwrap();
        let a = build_archive(r.path(), true).unwrap();
        assert!(a.dirty);
        assert_eq!(names(&a.tar), ["app/x.php"]);
        let out = Command::new("/usr/bin/tar")
            .args(["-xOf", "-", "app/x.php"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.take().unwrap().write_all(&a.tar)?;
                c.wait_with_output()
            })
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "changed\n");
    }

    #[test]
    fn release_name_format() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_794_798_245);
        let sha = "abc1234def5678abc1234def5678abc1234def56";
        assert_eq!(release_name(t, sha, false), "20261116T030405Z-abc1234");
        assert_eq!(release_name(t, sha, true), "20261116T030405Z-abc1234-dirty");
    }
}
