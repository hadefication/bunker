//! Host-side log files: the last N lines, and following a file as it grows.
use crate::error::{BunkerError, Result};
use std::io::{BufRead, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

/// The last `n` lines; a missing file has none.
pub fn last_lines(path: &Path, n: usize) -> Result<Vec<String>> {
    let text = match std::fs::read(path) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => {
            return Err(BunkerError::io(
                format!("cannot read {}", path.display()),
                e,
            ));
        }
    };
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    Ok(lines[lines.len().saturating_sub(n)..].to_vec())
}

/// Calls `on_line` for each line appended to `path` from now on, until interrupted. A truncated
/// file (log rotation) is read again from the start.
pub fn follow(path: &Path, on_line: &mut dyn FnMut(&str)) -> Result<()> {
    let open = || std::fs::File::open(path);
    let mut pos = open()
        .and_then(|f| f.metadata())
        .map(|m| m.len())
        .unwrap_or(0);
    loop {
        if let Ok(mut f) = open() {
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if len < pos {
                pos = 0;
            }
            if len > pos {
                f.seek(SeekFrom::Start(pos))
                    .map_err(|e| BunkerError::io(format!("cannot read {}", path.display()), e))?;
                let mut reader = std::io::BufReader::new(f);
                let mut buf = String::new();
                while reader.read_line(&mut buf).unwrap_or(0) > 0 {
                    if !buf.ends_with('\n') {
                        break;
                    }
                    pos += buf.len() as u64;
                    on_line(buf.trim_end_matches('\n'));
                    buf.clear();
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_lines_returns_the_tail() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.log");
        std::fs::write(&p, "a\nb\nc\nd\n").unwrap();
        assert_eq!(last_lines(&p, 2).unwrap(), ["c", "d"]);
        assert_eq!(last_lines(&p, 10).unwrap(), ["a", "b", "c", "d"]);
        assert!(
            last_lines(&d.path().join("missing.log"), 5)
                .unwrap()
                .is_empty()
        );
    }
}
