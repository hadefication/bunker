//! Host-side log rotation, run by the proxy's housekeeping tick: network.jsonl and every `*.log`
//! under `~/.bunker/logs` and `~/.bunker/<project>/logs`, at 10 MiB, keeping five copies.
use crate::net::log::{EventKind, append_event, read_events};
use std::path::{Path, PathBuf};

pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const KEEP: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// For files whose writers open them per write (network.jsonl, proxy.log): renamed, so no
    /// line is lost.
    Rename,
    /// For files a writer holds open (launchd's stdout/stderr for an agent): copied, then
    /// truncated in place, so the open descriptor keeps writing into the live file.
    CopyTruncate,
}

/// Rotates `path` once it reaches `max` bytes: `.4` → `.5` … `path` → `.1`.
pub fn rotate(path: &Path, max: u64, keep: u32, mode: Mode) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_file() && m.len() >= max => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    let numbered = |i: u32| {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{i}"));
        PathBuf::from(s)
    };
    let _ = std::fs::remove_file(numbered(keep));
    for i in (1..keep).rev() {
        if numbered(i).exists() {
            std::fs::rename(numbered(i), numbered(i + 1))?;
        }
    }
    match mode {
        Mode::Rename => std::fs::rename(path, numbered(1))?,
        Mode::CopyTruncate => {
            std::fs::copy(path, numbered(1))?;
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)?
                .set_len(0)?;
        }
    }
    Ok(true)
}

/// Rotates a project's network.jsonl and carries its newest policy event (Policy or PolicyError)
/// into the fresh file, so status and `net log` don't lose the policy state to a flood of
/// decisions.
fn rotate_network_log(path: &Path) {
    let newest = read_events(path).ok().and_then(|evs| {
        evs.into_iter().rev().find(|e| {
            matches!(
                e.kind,
                EventKind::Policy { .. } | EventKind::PolicyError { .. }
            )
        })
    });
    if rotate(path, MAX_BYTES, KEEP, Mode::Rename).unwrap_or(false)
        && let Some(e) = newest
    {
        let _ = append_event(path, &e);
    }
}

/// Every log bunker writes under `home`: `logs/*.log`, and per project `logs/*.log` and
/// `network.jsonl`. Failures are skipped; the next tick tries again.
pub fn rotate_all(home: &Path) {
    let logs_in = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "log"))
            .collect()
    };
    let mut files = logs_in(&home.join("logs"));
    for e in std::fs::read_dir(home).into_iter().flatten().flatten() {
        let dir = e.path();
        if dir.join("bunker.toml").is_file() {
            files.extend(logs_in(&dir.join("logs")));
            rotate_network_log(&dir.join("network.jsonl"));
        }
    }
    for f in files {
        // Agents' stdout/stderr (*.out.log, *.err.log) are held open by launchd.
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mode = if name.ends_with(".out.log") || name.ends_with(".err.log") {
            Mode::CopyTruncate
        } else {
            Mode::Rename
        };
        let _ = rotate(&f, MAX_BYTES, KEEP, mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_keeps_five_copies_at_ten_mib() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("proxy.log");
        std::fs::write(&p, vec![b'x'; 11 * 1024 * 1024]).unwrap();
        assert!(rotate(&p, MAX_BYTES, KEEP, Mode::CopyTruncate).unwrap());
        assert_eq!(
            std::fs::metadata(&p).unwrap().len(),
            0,
            "a fresh, empty file"
        );
        assert_eq!(
            std::fs::metadata(d.path().join("proxy.log.1"))
                .unwrap()
                .len(),
            11 * 1024 * 1024
        );
        for _ in 0..7 {
            std::fs::write(&p, vec![b'y'; 11 * 1024 * 1024]).unwrap();
            rotate(&p, MAX_BYTES, KEEP, Mode::CopyTruncate).unwrap();
        }
        for i in 1..=5 {
            assert!(d.path().join(format!("proxy.log.{i}")).exists(), ".{i}");
        }
        assert!(!d.path().join("proxy.log.6").exists());
        // Under the limit: untouched.
        std::fs::write(&p, b"small").unwrap();
        assert!(!rotate(&p, MAX_BYTES, KEEP, Mode::CopyTruncate).unwrap());
    }

    #[test]
    fn network_log_rotation_keeps_the_latest_policy_state() {
        // A guest can fill network.jsonl with blocked requests; the policy state status reads must
        // survive the rotation that flood triggers.
        use crate::net::log::{EventKind, NetEvent, append_event, last_policy};
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("t")).unwrap();
        std::fs::write(d.path().join("t/bunker.toml"), "").unwrap();
        let log = d.path().join("t/network.jsonl");
        append_event(
            &log,
            &NetEvent::now(
                "t",
                EventKind::PolicyError {
                    message: "bad toml".into(),
                },
            ),
        )
        .unwrap();
        let decision = serde_json::to_string(&NetEvent::now(
            "t",
            EventKind::Decision {
                host: "x.example".into(),
                port: 443,
                phase: "run".into(),
                verdict: "block".into(),
                reason: "not_listed".into(),
            },
        ))
        .unwrap()
            + "\n";
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        use std::io::Write;
        while f.metadata().unwrap().len() <= MAX_BYTES {
            f.write_all(decision.repeat(1000).as_bytes()).unwrap();
        }
        rotate_all(d.path());
        assert!(d.path().join("t/network.jsonl.1").exists());
        let kept = last_policy(&log).unwrap();
        assert!(kept.is_none(), "last_policy only reads Policy events");
        let events = crate::net::log::read_events(&log).unwrap();
        assert!(
            matches!(
                events.last().map(|e| &e.kind),
                Some(EventKind::PolicyError { .. })
            ),
            "the newest policy state is carried into the fresh file: {events:?}"
        );
    }

    #[test]
    fn rotate_all_covers_project_logs_and_network_log() {
        let d = tempfile::tempdir().unwrap();
        let big = vec![b'z'; (MAX_BYTES + 1) as usize];
        std::fs::create_dir_all(d.path().join("logs")).unwrap();
        std::fs::create_dir_all(d.path().join("t/logs")).unwrap();
        std::fs::write(d.path().join("logs/proxy.log"), &big).unwrap();
        std::fs::write(d.path().join("t/logs/tunnel.err.log"), &big).unwrap();
        std::fs::write(d.path().join("t/network.jsonl"), &big).unwrap();
        std::fs::write(d.path().join("t/bunker.toml"), &big).unwrap();
        rotate_all(d.path());
        for f in [
            "logs/proxy.log.1",
            "t/logs/tunnel.err.log.1",
            "t/network.jsonl.1",
        ] {
            assert!(d.path().join(f).exists(), "{f}");
        }
        assert!(
            !d.path().join("t/bunker.toml.1").exists(),
            "only logs rotate"
        );
    }
}
