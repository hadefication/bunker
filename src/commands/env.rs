//! `bunker env set|unset|list`. Values are read without echo (or from stdin), go to the Keychain,
//! and reach the guest only on a child's stdin. No command prints a value.
use crate::config::validate::validate_env_key;
use crate::config::{Config, SecretKind, resolve_project_in};
use crate::deploy::steps::recache_script;
use crate::error::{BunkerError, Result};
use crate::host::Host;
use crate::lima::vm::{instance_status, shell, vm_name};
use crate::output::{ExitCode, Report};
use crate::secrets::dotenv::{MANAGED_FIXED, PROTECTED, render_env};
use crate::secrets::meta::{Entry, record};
use crate::secrets::store::SecretStore;
use serde_json::{Value, json};
use std::io::{self, Write};
use zeroize::Zeroizing;

pub enum EnvArgs {
    Set {
        key: String,
        rest: Vec<String>,
        stdin: bool,
        plain: bool,
        bind: Option<String>,
    },
    Unset {
        key: String,
        project: Option<String>,
    },
    List {
        project: Option<String>,
    },
}

/// Validated arguments with the value already read, so nothing system-touching runs first.
pub enum Prepared {
    SetSecret {
        key: String,
        project: Option<String>,
        value: Zeroizing<Vec<u8>>,
    },
    SetPlain {
        key: String,
        project: Option<String>,
        value: String,
    },
    Unset {
        key: String,
        project: Option<String>,
    },
    List {
        project: Option<String>,
    },
}

/// Validates and reads the value: `--stdin`, else a no-echo TTY prompt, else `needs_tty`.
pub fn prepare(
    a: EnvArgs,
    read_stdin: &dyn Fn() -> io::Result<Vec<u8>>,
    tty: bool,
) -> Result<Prepared> {
    let refused = |code, msg: String| BunkerError::new(code, ExitCode::Refused, msg);
    match a {
        EnvArgs::Set {
            key,
            rest,
            stdin,
            plain,
            bind,
        } => {
            validate_env_key(&key)?;
            if bind.is_some() {
                return Err(refused(
                    "not_yet_supported",
                    "outbound credentials arrive in a later release".into(),
                )
                .hint("store it as an in-app secret for now: drop --bind"));
            }
            if plain {
                if PROTECTED.contains(&key.as_str()) {
                    return Err(refused(
                        "key_protected",
                        format!("{key} is managed by bunker and can't be a plain setting"),
                    ));
                }
                let mut it = rest.into_iter();
                let value = it.next().ok_or_else(|| {
                    BunkerError::new(
                        "invalid_value",
                        ExitCode::Usage,
                        "env set KEY VALUE --plain needs a value",
                    )
                })?;
                if value.contains(['\n', '\r', '\0']) {
                    return Err(BunkerError::new(
                        "invalid_value",
                        ExitCode::Usage,
                        "plain settings are single-line",
                    ));
                }
                return Ok(Prepared::SetPlain {
                    key,
                    project: it.next(),
                    value,
                });
            }
            if MANAGED_FIXED.contains(&key.as_str()) {
                return Err(refused(
                    "key_protected",
                    format!("{key} is managed by bunker"),
                ));
            }
            if rest.len() > 1 {
                return Err(BunkerError::new(
                    "invalid_value",
                    ExitCode::Usage,
                    "a secret's value is never an argument",
                )
                .hint("type it when asked, or pipe it with --stdin"));
            }
            let project = rest.into_iter().next();
            let mut value = if stdin {
                Zeroizing::new(read_stdin().map_err(|e| BunkerError::io("cannot read stdin", e))?)
            } else if tty {
                let v = rpassword::prompt_password(format!("Value for {key} (not echoed): "))
                    .map_err(|e| BunkerError::io("cannot read the value", e))?;
                Zeroizing::new(v.into_bytes())
            } else {
                return Err(refused("needs_tty", format!("{key} needs a value"))
                    .hint("run it in a terminal to type it, or pipe it with --stdin"));
            };
            if value.last() == Some(&b'\n') {
                value.pop();
            }
            if value.is_empty() {
                return Err(refused(
                    "empty_value",
                    format!("{key} would be empty; nothing stored"),
                ));
            }
            Ok(Prepared::SetSecret {
                key,
                project,
                value,
            })
        }
        EnvArgs::Unset { key, project } => {
            validate_env_key(&key)?;
            Ok(Prepared::Unset { key, project })
        }
        EnvArgs::List { project } => Ok(Prepared::List { project }),
    }
}

#[derive(Debug)]
pub enum EnvReport {
    Changed {
        project: String,
        key: String,
        kind: &'static str,
        action: &'static str,
        note: String,
    },
    List {
        project: String,
        entries: Vec<(String, String, Option<bool>)>,
    },
}

impl Report for EnvReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        match self {
            EnvReport::Changed {
                project,
                key,
                kind,
                action,
                note,
            } => {
                writeln!(w, "{project}: {key} {action} ({kind})")?;
                writeln!(w, "  {note}")
            }
            EnvReport::List { project, entries } => {
                if entries.is_empty() {
                    return writeln!(w, "{project}: no settings or secrets");
                }
                for (k, kind, stored) in entries {
                    let state = match stored {
                        Some(true) => "  stored",
                        Some(false) => "  MISSING from the Keychain",
                        None => "",
                    };
                    writeln!(w, "{k:<28} {kind}{state}")?;
                }
                Ok(())
            }
        }
    }
    fn json(&self) -> Value {
        match self {
            EnvReport::Changed {
                project,
                key,
                kind,
                action,
                note,
            } => {
                json!({"project": project, "key": key, "kind": kind, "action": action, "note": note})
            }
            EnvReport::List { project, entries } => json!({
                "project": project,
                "entries": entries.iter().map(|(k, kind, stored)| json!({"key": k, "kind": kind, "stored": stored})).collect::<Vec<_>>(),
            }),
        }
    }
}

pub fn run(h: &Host, store: &dyn SecretStore, p: Prepared) -> Result<EnvReport> {
    let cwd = std::env::current_dir()
        .map_err(|e| BunkerError::io("cannot read the current directory", e))?;
    let project_arg = match &p {
        Prepared::SetSecret { project, .. }
        | Prepared::SetPlain { project, .. }
        | Prepared::Unset { project, .. }
        | Prepared::List { project } => project.clone(),
    };
    let pd = resolve_project_in(&h.home, project_arg.as_deref(), &cwd, &mut io::stderr())?;
    let cfg = Config::load(&pd)?;
    let (key, kind, action) = match p {
        Prepared::List { .. } => {
            let mut entries: Vec<(String, String, Option<bool>)> = cfg
                .env
                .keys()
                .map(|k| (k.clone(), "plain".to_string(), None))
                .collect();
            for (k, meta) in &cfg.secrets {
                let kind = match (meta.kind, &meta.host) {
                    (SecretKind::Bound, Some(host)) => format!("bound to {host}"),
                    _ => "in_app".to_string(),
                };
                entries.push((k.clone(), kind, Some(store.get(&pd.name, k)?.is_some())));
            }
            return Ok(EnvReport::List {
                project: pd.name,
                entries,
            });
        }
        Prepared::SetPlain { key, value, .. } => {
            if cfg.secrets.contains_key(&key) {
                return Err(BunkerError::new(
                    "key_conflict",
                    ExitCode::Refused,
                    format!("{key} is a secret"),
                )
                .hint(format!("bunker env unset {key} {} first", pd.name)));
            }
            record(&pd, &key, Entry::Plain(&value))?;
            (key, "plain", "set")
        }
        Prepared::SetSecret { key, value, .. } => {
            if cfg.env.contains_key(&key) {
                return Err(BunkerError::new(
                    "key_conflict",
                    ExitCode::Refused,
                    format!("{key} is a plain setting"),
                )
                .hint(format!("bunker env unset {key} {} first", pd.name)));
            }
            store.set(&pd.name, &key, &value)?;
            record(&pd, &key, Entry::Secret(SecretKind::InApp))?;
            (key, "in_app", "set")
        }
        Prepared::Unset { key, .. } => {
            let was_secret = cfg.secrets.contains_key(&key);
            let deleted = store.delete(&pd.name, &key)?;
            if !was_secret && !deleted && !cfg.env.contains_key(&key) {
                return Ok(EnvReport::Changed {
                    project: pd.name,
                    key,
                    kind: "none",
                    action: "was not set",
                    note: "nothing changed".into(),
                });
            }
            record(&pd, &key, Entry::Remove)?;
            (key, if was_secret { "in_app" } else { "plain" }, "unset")
        }
    };
    let cfg = Config::load(&pd)?;
    let note = match render_env(&pd.name, &cfg, store) {
        Ok(rendered) => push_env(h, &pd.name, &rendered)?,
        Err(e) => format!("not pushed to the VM: {}", e.message),
    };
    Ok(EnvReport::Changed {
        project: pd.name,
        key,
        kind,
        action,
        note,
    })
}

/// Installs the rendered `.env` into the guest from stdin; when a release is live, re-caches
/// config and restarts the workers. Returns what happened, for the report.
pub fn push_env(h: &Host, project: &str, rendered: &str) -> Result<String> {
    let vm = vm_name(project);
    if instance_status(h, &vm)?.as_deref() != Some("Running") {
        return Ok("the VM isn't running; it gets the new .env on the next deploy".into());
    }
    if !h
        .r
        .run(&shell(h, &vm).args(["test", "-d", "/srv/app/shared"]))?
        .success()
    {
        return Ok("the VM isn't provisioned yet; it gets the new .env on the next deploy".into());
    }
    h.r.run_ok(
        &shell(h, &vm)
            .args([
                "sudo",
                "install",
                "-m",
                "0640",
                "-o",
                "root",
                "-g",
                "app",
                "/dev/stdin",
                "/srv/app/shared/.env",
            ])
            .stdin(rendered.as_bytes().to_vec()),
    )?;
    if !h
        .r
        .run(&shell(h, &vm).args(["test", "-e", "/srv/app/current"]))?
        .success()
    {
        return Ok("wrote shared/.env in the VM".into());
    }
    h.r.run_ok(
        &shell(h, &vm)
            .args(["sudo", "-u", "app", "bash", "-s"])
            .stdin(recache_script("/srv/app/current").into_bytes()),
    )?;
    h.r.run_ok(&shell(h, &vm).args([
        "sudo",
        "systemctl",
        "try-restart",
        "bunker-worker-*.service",
    ]))?;
    Ok("wrote shared/.env in the VM, re-cached config and restarted the workers".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, host, project};
    use crate::run::{Output, RecordingRunner};
    use crate::secrets::store::MemoryStore;

    const SECRET: &str = "s3cr3t-fixture-value";

    fn running(r: &RecordingRunner) {
        r.respond(
            "limactl",
            &["list"],
            Output::ok("{\"name\":\"bunker-t\",\"status\":\"Running\"}\n"),
        );
    }

    fn set_secret(key: &str) -> Prepared {
        Prepared::SetSecret {
            key: key.into(),
            project: Some("t".into()),
            value: Zeroizing::new(SECRET.as_bytes().to_vec()),
        }
    }

    #[test]
    fn push_env_sends_values_on_stdin_only() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        running(&r);
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let store = MemoryStore::default();
        let rep = run(&h, &store, set_secret("API_TOKEN")).unwrap();
        r.assert_no_secret_in_argv(&[SECRET]);
        let install = r
            .calls()
            .into_iter()
            .find(|c| c.args.iter().any(|a| a == "/srv/app/shared/.env"))
            .expect("install call");
        assert_eq!(
            install.args[5..],
            [
                "sudo",
                "install",
                "-m",
                "0640",
                "-o",
                "root",
                "-g",
                "app",
                "/dev/stdin",
                "/srv/app/shared/.env"
            ]
        );
        assert!(
            String::from_utf8_lossy(install.stdin.as_deref().unwrap())
                .contains(&format!("API_TOKEN=\"{SECRET}\""))
        );
        assert!(
            format!("{rep:?}").find(SECRET).is_none(),
            "the report carries no value"
        );
        assert_eq!(
            store.get("t", "API_TOKEN").unwrap().unwrap().as_slice(),
            SECRET.as_bytes()
        );
    }

    #[test]
    fn push_env_recaches_as_app_and_locks_the_cache() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        running(&r);
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        push_env(&h, "t", "A=\"b\"\n").unwrap();
        assert!(r.calls().iter().any(|c| {
            c.args.windows(3).any(|w| w == ["sudo", "-u", "app"])
                && c.args
                    .iter()
                    .chain(std::iter::once(
                        &String::from_utf8_lossy(c.stdin.as_deref().unwrap_or_default())
                            .into_owned(),
                    ))
                    .any(|a| {
                        a.contains(
                            "php artisan config:cache\nsetfacl -b bootstrap/cache/config.php\nchmod 0600 bootstrap/cache/config.php",
                        )
                    })
        }));
    }

    #[test]
    fn env_list_never_prints_values() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let store = MemoryStore::default();
        run(&h, &store, set_secret("API_TOKEN")).unwrap();
        run(
            &h,
            &store,
            Prepared::SetPlain {
                key: "MAIL_MAILER".into(),
                project: Some("t".into()),
                value: "log".into(),
            },
        )
        .unwrap();
        let rep = run(
            &h,
            &store,
            Prepared::List {
                project: Some("t".into()),
            },
        )
        .unwrap();
        let mut human = Vec::new();
        rep.human(&mut human).unwrap();
        let human = String::from_utf8(human).unwrap();
        let json = rep.json().to_string();
        for out in [&human, &json] {
            assert!(
                !out.contains(SECRET) && !out.contains("\"log\"") && !out.contains(" log"),
                "{out}"
            );
            assert!(
                out.contains("API_TOKEN") && out.contains("MAIL_MAILER"),
                "{out}"
            );
        }
        assert!(
            human.contains("in_app") && human.contains("plain"),
            "{human}"
        );
    }

    #[test]
    fn unset_removes_secret_from_store_and_config() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        let pd = project(&h, "t");
        let store = MemoryStore::default();
        run(&h, &store, set_secret("API_TOKEN")).unwrap();
        run(
            &h,
            &store,
            Prepared::Unset {
                key: "API_TOKEN".into(),
                project: Some("t".into()),
            },
        )
        .unwrap();
        assert!(store.get("t", "API_TOKEN").unwrap().is_none());
        assert!(
            !std::fs::read_to_string(pd.config())
                .unwrap()
                .contains("API_TOKEN")
        );
    }

    #[test]
    fn stopped_vm_is_not_pushed_to() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let rep = run(&h, &MemoryStore::default(), set_secret("API_TOKEN")).unwrap();
        assert!(
            !r.calls()
                .iter()
                .any(|c| c.args.iter().any(|a| a == "/srv/app/shared/.env"))
        );
        assert!(format!("{rep:?}").contains("next"), "{rep:?}");
    }

    #[test]
    fn prepare_reads_stdin_and_refuses_empty() {
        let args = |stdin| EnvArgs::Set {
            key: "API_TOKEN".into(),
            rest: vec!["t".into()],
            stdin,
            plain: false,
            bind: None,
        };
        match prepare(args(true), &|| Ok(b"abc\n".to_vec()), false).unwrap() {
            Prepared::SetSecret { value, project, .. } => assert_eq!(
                (value.as_slice(), project.as_deref()),
                (&b"abc"[..], Some("t"))
            ),
            _ => panic!(),
        }
        assert_eq!(
            prepare(args(true), &|| Ok(b"\n".to_vec()), false)
                .err()
                .unwrap()
                .code,
            "empty_value"
        );
        assert_eq!(
            prepare(args(false), &|| Ok(vec![]), false)
                .err()
                .unwrap()
                .code,
            "needs_tty"
        );
    }
}
