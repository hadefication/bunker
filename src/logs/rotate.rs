//! Host-side log rotation, run by the proxy's housekeeping tick: network.jsonl and every `*.log`
//! under `~/.bunker/logs` and `~/.bunker/<project>/logs`, at 10 MiB, keeping five copies.
use std::path::Path;

pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const KEEP: u32 = 5;

/// Rotates `path` once it reaches `max` bytes: `.4` → `.5` … `path` → `.1` (copied, then the
/// file truncated in place, so a writer holding it open, like launchd for an agent's stdout,
/// keeps writing into the live file).
pub fn rotate(path: &Path, max: u64, keep: u32) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_file() && m.len() >= max => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    let numbered = |i: u32| {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{i}"));
        std::path::PathBuf::from(s)
    };
    let _ = std::fs::remove_file(numbered(keep));
    for i in (1..keep).rev() {
        if numbered(i).exists() {
            std::fs::rename(numbered(i), numbered(i + 1))?;
        }
    }
    std::fs::copy(path, numbered(1))?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_len(0)?;
    Ok(true)
}

/// Every log bunker writes under `home`: `logs/*.log`, and per project `logs/*.log` and
/// `network.jsonl`. Failures are skipped; the next tick tries again.
pub fn rotate_all(home: &Path) {
    let logs_in = |dir: &Path| -> Vec<std::path::PathBuf> {
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
            files.push(dir.join("network.jsonl"));
        }
    }
    for f in files {
        let _ = rotate(&f, MAX_BYTES, KEEP);
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
        assert!(rotate(&p, MAX_BYTES, KEEP).unwrap());
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
            rotate(&p, MAX_BYTES, KEEP).unwrap();
        }
        for i in 1..=5 {
            assert!(d.path().join(format!("proxy.log.{i}")).exists(), ".{i}");
        }
        assert!(!d.path().join("proxy.log.6").exists());
        // Under the limit: untouched.
        std::fs::write(&p, b"small").unwrap();
        assert!(!rotate(&p, MAX_BYTES, KEEP).unwrap());
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
