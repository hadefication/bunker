//! `bunker net allow|deny|rm|reload|log`. Agents may run these: the control is the printed diff
//! and the `network.jsonl` entry every change leaves, not a lock (see the spec's allowlist edits).
use crate::config::validate::validate_host_pattern;
use crate::config::{Config, resolve_project};
use crate::error::{BunkerError, Result};
use crate::net::edit::{Change, ListName, Op, edit_lists, normalize_host};
use crate::net::log::{EventKind, NetEvent, append_event, last_policy, read_events};
use crate::output::Report;
use crate::paths::{ProjectDir, write_private};
use serde_json::{Value, json};
use std::io::{self, Write};

pub enum NetArgs {
    Allow {
        host: String,
        project: Option<String>,
        build: bool,
    },
    Deny {
        host: String,
        project: Option<String>,
    },
    Rm {
        host: String,
        project: Option<String>,
    },
    Reload {
        project: Option<String>,
    },
    Log {
        project: Option<String>,
        blocked: bool,
        n: usize,
    },
}

pub enum NetReport {
    Changes {
        project: String,
        verb: &'static str,
        host: Option<String>,
        changes: Vec<Change>,
    },
    Log {
        project: String,
        events: Vec<NetEvent>,
    },
}

impl Report for NetReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        match self {
            NetReport::Changes {
                project,
                verb,
                host,
                changes,
            } => {
                let changed: Vec<&Change> =
                    changes.iter().filter(|c| c.before != c.after).collect();
                if changed.is_empty() {
                    let what = match (*verb, host) {
                        ("allow", Some(h)) => format!("{h} is already allowed"),
                        ("deny", Some(h)) => format!("{h} is already denied"),
                        (_, Some(h)) => format!("{h} is not listed"),
                        _ => "no changes since the last policy".to_string(),
                    };
                    return writeln!(w, "{project}: {what}");
                }
                writeln!(w, "{project}: [network] changed")?;
                for c in changed {
                    for h in c.before.iter().filter(|h| !c.after.contains(h)) {
                        writeln!(w, "  - {h}  ({})", c.list.key())?;
                    }
                    for h in c.after.iter().filter(|h| !c.before.contains(h)) {
                        writeln!(w, "  + {h}  ({})", c.list.key())?;
                    }
                }
                writeln!(
                    w,
                    "the proxy picks this up within a second; logged to network.jsonl"
                )
            }
            NetReport::Log { project, events } => {
                if events.is_empty() {
                    return writeln!(w, "{project}: no matching events");
                }
                for e in events {
                    writeln!(w, "{}", describe(e))?;
                }
                Ok(())
            }
        }
    }

    fn json(&self) -> Value {
        match self {
            NetReport::Changes {
                project, changes, ..
            } => json!({"project": project, "changes": changes}),
            NetReport::Log { project, events } => json!({"project": project, "events": events}),
        }
    }
}

/// Escapes control characters (ESC, C1, newlines). Hosts in decision events come from the guest's
/// own requests, so a compromised VM must not be able to drive the operator's terminal.
fn clean(s: &str) -> String {
    crate::output::printable(s).into_owned()
}

fn describe(e: &NetEvent) -> String {
    clean(&describe_raw(e))
}

fn describe_raw(e: &NetEvent) -> String {
    match &e.kind {
        EventKind::Decision {
            host,
            port,
            phase,
            verdict,
            reason,
        } => {
            format!("{}  {verdict:<5}  {host}:{port}  ({reason}, {phase})", e.ts)
        }
        EventKind::Policy {
            source,
            allow,
            build,
            deny,
        } => format!(
            "{}  policy ({source})  allow [{}]  build [{}]  deny [{}]",
            e.ts,
            allow.join(", "),
            build.join(", "),
            deny.join(", ")
        ),
        EventKind::PolicyError { message } => format!("{}  policy error  {message}", e.ts),
        EventKind::Lease {
            lease_kind,
            action,
            ttl_secs,
        } => match ttl_secs {
            Some(t) => format!("{}  lease {lease_kind} {action} ({t}s)", e.ts),
            None => format!("{}  lease {lease_kind} {action}", e.ts),
        },
    }
}

pub fn run(a: NetArgs) -> Result<NetReport> {
    let cwd = std::env::current_dir()
        .map_err(|e| BunkerError::io("cannot read the current directory", e))?;
    match a {
        NetArgs::Allow {
            host,
            project,
            build,
        } => {
            let target = if build {
                ListName::Build
            } else {
                ListName::Allow
            };
            edit(
                project,
                &cwd,
                "allow",
                &host,
                &[(target, Op::Add), (ListName::Deny, Op::Remove)],
            )
        }
        NetArgs::Deny { host, project } => edit(
            project,
            &cwd,
            "deny",
            &host,
            &[
                (ListName::Deny, Op::Add),
                (ListName::Allow, Op::Remove),
                (ListName::Build, Op::Remove),
            ],
        ),
        NetArgs::Rm { host, project } => edit(
            project,
            &cwd,
            "rm",
            &host,
            &[
                (ListName::Allow, Op::Remove),
                (ListName::Build, Op::Remove),
                (ListName::Deny, Op::Remove),
            ],
        ),
        NetArgs::Reload { project } => reload(&resolve_project(project.as_deref(), &cwd)?),
        NetArgs::Log {
            project,
            blocked,
            n,
        } => {
            let pd = resolve_project(project.as_deref(), &cwd)?;
            let mut events: Vec<NetEvent> = read_events(&pd.network_log())?
                .into_iter()
                .filter(|e| !blocked || matches!(&e.kind, EventKind::Decision { verdict, .. } if verdict == "block"))
                .collect();
            let skip = events.len().saturating_sub(n);
            events.drain(..skip);
            Ok(NetReport::Log {
                project: pd.name,
                events,
            })
        }
    }
}

fn edit(
    project: Option<String>,
    cwd: &std::path::Path,
    verb: &'static str,
    host: &str,
    ops: &[(ListName, Op)],
) -> Result<NetReport> {
    validate_host_pattern(host)?;
    let host = normalize_host(host);
    let pd = resolve_project(project.as_deref(), cwd)?;
    let path = pd.config();
    let before = std::fs::read_to_string(&path)
        .map_err(|e| BunkerError::io(format!("cannot read {}", path.display()), e))?;
    let edits: Vec<(ListName, Op, &str)> =
        ops.iter().map(|&(l, o)| (l, o, host.as_str())).collect();
    let (after, changes) = edit_lists(&before, &edits).map_err(|mut e| {
        e.message = e
            .message
            .replacen("bunker.toml", &path.display().to_string(), 1);
        e.hint = Some(format!(
            "fix {} by hand, then run bunker net reload",
            path.display()
        ));
        e
    })?;
    if changes.iter().any(|c| c.before != c.after) {
        write_private(&path, after.as_bytes())?;
        log_policy(&pd, "edit", &Config::parse(&after)?)?;
    }
    Ok(NetReport::Changes {
        project: pd.name,
        verb,
        host: Some(host),
        changes,
    })
}

fn reload(pd: &ProjectDir) -> Result<NetReport> {
    let cfg = match Config::load(pd) {
        Ok(c) => c,
        Err(e) if e.code == "config_invalid" => {
            append_event(
                &pd.network_log(),
                &NetEvent::now(
                    &pd.name,
                    EventKind::PolicyError {
                        message: e.message.clone(),
                    },
                ),
            )?;
            return Err(
                e.hint("the proxy blocks all of this project's traffic until the file is valid")
            );
        }
        Err(e) => return Err(e),
    };
    let prev = match last_policy(&pd.network_log())? {
        Some(NetEvent {
            kind: EventKind::Policy {
                allow, build, deny, ..
            },
            ..
        }) => [allow, build, deny],
        _ => [vec![], vec![], vec![]],
    };
    let now = [
        cfg.network.allow.clone(),
        cfg.network.build.clone(),
        cfg.network.deny.clone(),
    ];
    let changes = [ListName::Allow, ListName::Build, ListName::Deny]
        .into_iter()
        .zip(prev.into_iter().zip(now))
        .map(|(list, (before, after))| Change {
            list,
            before,
            after,
        })
        .collect();
    log_policy(pd, "reload", &cfg)?;
    Ok(NetReport::Changes {
        project: pd.name.clone(),
        verb: "reload",
        host: None,
        changes,
    })
}

fn log_policy(pd: &ProjectDir, source: &str, cfg: &Config) -> Result<()> {
    append_event(
        &pd.network_log(),
        &NetEvent::now(
            &pd.name,
            EventKind::Policy {
                source: source.to_string(),
                allow: cfg.network.allow.clone(),
                build: cfg.network.build.clone(),
                deny: cfg.network.deny.clone(),
            },
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_lines_escape_guest_controlled_text() {
        let e = NetEvent::now(
            "p",
            EventKind::Decision {
                host: "evil\u{1b}]0;pwned\u{7}\u{9b}31m.example\nfake line".into(),
                port: 443,
                phase: "run".into(),
                verdict: "block".into(),
                reason: "not_listed".into(),
            },
        );
        let line = describe(&e);
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert!(line.contains("evil\\u{1b}]0;pwned"), "{line}");
    }
}
