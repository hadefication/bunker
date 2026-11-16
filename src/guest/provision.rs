//! `bunker up` phase B: provision the guest inside a Build lease.
use crate::config::Config;
use crate::config::detect::detect;
use crate::error::{BunkerError, Result};
use crate::guest::render::render_provision;
use crate::host::Host;
use crate::lima::vm::{shell, vm_name};
use crate::output::ExitCode;
use crate::paths::{ProjectDir, ensure_private_dir, write_private};
use crate::proxy::lease::{LeaseKind, open_lease};
use crate::secrets::generate::generate_missing;
use crate::secrets::store::SecretStore;
use std::time::Duration;

/// Package installs on a first provision take minutes; the lease bounds how long the build hosts
/// stay open if bunker dies mid-way.
const BUILD_LEASE: Duration = Duration::from_secs(45 * 60);

/// What `bash -s` reads: a `read` that takes the next line, the password on that line, then the
/// script. bash reads a piped script a byte at a time, so the `read` gets exactly that line.
pub fn stdin_for(password: &str, script: &str) -> Vec<u8> {
    format!("IFS= read -r BUNKER_DB_PASSWORD\n{password}\n{script}").into_bytes()
}

/// Generates missing in-app secrets, then runs the provisioning script as root in the guest while a
/// Build lease opens the package hosts. Its output goes to `logs/provision.log`.
pub fn provision(h: &Host, store: &dyn SecretStore, pd: &ProjectDir, cfg: &Config) -> Result<()> {
    let passport = detect(&cfg.project)?.passport;
    generate_missing(pd, cfg, passport, store, h.r)?;
    let pw = store.get(&pd.name, "DB_PASSWORD")?.ok_or_else(|| {
        BunkerError::new(
            "secret_missing",
            ExitCode::Failure,
            "DB_PASSWORD is not in the Keychain after generating it",
        )
    })?;
    let pw = std::str::from_utf8(&pw).map_err(|_| {
        BunkerError::new(
            "secret_invalid",
            ExitCode::Failure,
            "DB_PASSWORD is not UTF-8",
        )
    })?;
    let script = render_provision(cfg);
    let out = {
        let _lease = open_lease(pd, LeaseKind::Build, BUILD_LEASE)?;
        h.r.run(
            &shell(h, &vm_name(&pd.name))
                .args(["sudo", "bash", "-s"])
                .stdin(stdin_for(pw, &script)),
        )?
    };
    let mut log = out.stdout.clone();
    log.extend_from_slice(&out.stderr);
    ensure_private_dir(&pd.logs_dir())?;
    write_private(&pd.logs_dir().join("provision.log"), &log)?;
    if !out.success() {
        let stderr = out.stderr_str();
        let lines: Vec<&str> = stderr.lines().collect();
        let tail = lines[lines.len().saturating_sub(5)..].join("\n");
        return Err(BunkerError::new(
            "provision_failed",
            ExitCode::Failure,
            format!(
                "provisioning {} failed (exit {}): {tail}",
                pd.name, out.status
            ),
        )
        .hint(format!(
            "full output in {}; blocked hosts: bunker net log {}",
            pd.logs_dir().join("provision.log").display(),
            pd.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, host, project};
    use crate::proxy::lease::{LeaseKind, lease_active};
    use crate::run::{Cmd, Output, RecordingRunner, Runner};
    use crate::secrets::store::MemoryStore;
    use std::sync::{Arc, Mutex};

    fn is_provision(c: &Cmd) -> bool {
        c.program == "limactl"
            && c.args
                .ends_with(&["sudo".into(), "bash".into(), "-s".into()])
    }

    #[test]
    fn provision_runs_inside_build_lease() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        let pd = project(&h, "t");
        let seen = Arc::new(Mutex::new(None));
        let (pd2, seen2) = (pd.clone(), seen.clone());
        r.respond_with("limactl", &["shell"], move |c| {
            if is_provision(c) {
                *seen2.lock().unwrap() = Some(lease_active(
                    &pd2,
                    LeaseKind::Build,
                    std::time::SystemTime::now(),
                ));
            }
            Output::ok("")
        });
        let cfg = Config::load(&pd).unwrap();
        provision(&h, &MemoryStore::default(), &pd, &cfg).unwrap();
        assert_eq!(
            *seen.lock().unwrap(),
            Some(true),
            "lease open during the bash call"
        );
        assert!(!lease_active(
            &pd,
            LeaseKind::Build,
            std::time::SystemTime::now()
        ));
    }

    #[test]
    fn provision_sends_db_password_on_stdin_only() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        let h = host(&r, &CONFINED, d.path());
        let pd = project(&h, "t");
        let store = MemoryStore::default();
        let cfg = Config::load(&pd).unwrap();
        provision(&h, &store, &pd, &cfg).unwrap();
        let pw = store.get("t", "DB_PASSWORD").unwrap().expect("generated");
        let pw = String::from_utf8(pw.to_vec()).unwrap();
        r.assert_no_secret_in_argv(&[&pw]);
        let call = r
            .calls()
            .into_iter()
            .find(is_provision)
            .expect("provision call");
        let stdin = String::from_utf8(call.stdin.unwrap()).unwrap();
        assert!(stdin.starts_with(&format!("IFS= read -r BUNKER_DB_PASSWORD\n{pw}\n")));
        assert_eq!(
            stdin.matches(&pw).count(),
            1,
            "the password appears once, on its own line"
        );
    }

    #[test]
    fn stdin_layout_delivers_the_password_to_the_script() {
        let body = "set -euo pipefail\nf() {\nprintf 'got:%s' \"$BUNKER_DB_PASSWORD\"\ncat\n}\nf </dev/null\n";
        let out = crate::run::SystemRunner
            .run(
                &Cmd::new("/bin/bash")
                    .arg("-s")
                    .stdin(stdin_for("pw123", body)),
            )
            .unwrap();
        assert!(out.success(), "{}", out.stderr_str());
        assert_eq!(
            out.stdout_str(),
            "got:pw123",
            "the body must not read the rest of stdin"
        );
    }
}
