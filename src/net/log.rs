//! `network.jsonl`: one JSON object per line for every proxy decision, policy change and lease.
use crate::error::{BunkerError, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetEvent {
    pub ts: String,
    pub project: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    Decision {
        host: String,
        port: u16,
        phase: String,
        verdict: String,
        reason: String,
    },
    Policy {
        source: String,
        allow: Vec<String>,
        build: Vec<String>,
        deny: Vec<String>,
    },
    PolicyError {
        message: String,
    },
    Lease {
        #[serde(rename = "lease")]
        lease_kind: String,
        action: String,
        #[serde(default)]
        ttl_secs: Option<u64>,
    },
}

impl NetEvent {
    pub fn now(project: &str, kind: EventKind) -> Self {
        Self {
            ts: crate::time::rfc3339(std::time::SystemTime::now()),
            project: project.to_string(),
            kind,
        }
    }
}

/// Where the newest policy event is also kept, beside `network.jsonl`, so rotation can carry it
/// forward without reading the log.
pub fn policy_state_path(log: &Path) -> std::path::PathBuf {
    log.with_file_name("policy-state.json")
}

/// Appends one line with `O_APPEND`, so concurrent writers (proxy, CLI) never interleave a line.
/// A policy event is also written to `policy-state.json`. Crossing 10 MiB rotates the log right
/// away: a guest flooding blocked requests can't grow it between the proxy's housekeeping ticks.
pub fn append_event(path: &Path, e: &NetEvent) -> Result<()> {
    let mut line = serde_json::to_vec(e).expect("events serialize");
    line.push(b'\n');
    let len = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| {
            f.write_all(&line)?;
            f.metadata().map(|m| m.len())
        })
        .map_err(|err| BunkerError::io(format!("cannot append to {}", path.display()), err))?;
    if matches!(
        e.kind,
        EventKind::Policy { .. } | EventKind::PolicyError { .. }
    ) {
        crate::paths::write_private(&policy_state_path(path), &line)?;
    }
    if len >= crate::logs::rotate::MAX_BYTES {
        crate::logs::rotate::rotate_network_log(path);
    }
    Ok(())
}

/// The newest `policy` event, if any.
pub fn last_policy(path: &Path) -> Result<Option<NetEvent>> {
    Ok(read_events(path)?
        .into_iter()
        .rev()
        .find(|e| matches!(e.kind, EventKind::Policy { .. })))
}

/// Every parseable event, oldest first. Lines that don't parse are skipped; a missing file is empty.
pub fn read_events(path: &Path) -> Result<Vec<NetEvent>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(BunkerError::io(
            format!("cannot read {}", path.display()),
            e,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn policy(src: &str, allow: &[&str]) -> NetEvent {
        NetEvent::now(
            "p",
            EventKind::Policy {
                source: src.into(),
                allow: allow.iter().map(|s| s.to_string()).collect(),
                build: vec![],
                deny: vec![],
            },
        )
    }

    #[test]
    fn append_creates_0600_and_last_policy_finds_newest() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("network.jsonl");
        assert_eq!(last_policy(&p).unwrap(), None);
        append_event(&p, &policy("edit", &["a.example"])).unwrap();
        append_event(
            &p,
            &NetEvent::now(
                "p",
                EventKind::PolicyError {
                    message: "x".into(),
                },
            ),
        )
        .unwrap();
        std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        append_event(&p, &policy("reload", &["b.example"])).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let got = last_policy(&p).unwrap().unwrap();
        assert!(matches!(got.kind, EventKind::Policy { ref source, .. } if source == "reload"));
        let line = std::fs::read_to_string(&p)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_string();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            (
                v["kind"].as_str(),
                v["source"].as_str(),
                v["project"].as_str()
            ),
            (Some("policy"), Some("edit"), Some("p"))
        );
        assert!(v["ts"].as_str().unwrap().ends_with('Z'));
    }
}
