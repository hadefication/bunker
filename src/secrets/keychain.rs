//! macOS Keychain generic passwords, service `bunker.<project>`, account = the env key. Items are
//! created by bunker, so the legacy keychain's default access lets bunker read them without a
//! prompt and makes every other process (an agent's `security find-generic-password`) ask.
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use crate::secrets::store::{Secret, SecretStore};
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::os::macos::passwords::find_generic_password;
use std::path::PathBuf;
use zeroize::Zeroizing;

const NOT_FOUND: i32 = -25300; // errSecItemNotFound

fn service(project: &str) -> String {
    format!("bunker.{project}")
}

fn kc_error(what: &str, key: &str, e: security_framework::base::Error) -> BunkerError {
    BunkerError::new(
        "keychain",
        ExitCode::Failure,
        format!("Keychain {what} {key}: {e}"),
    )
}

pub struct KeychainStore {
    /// `BUNKER_KEYCHAIN`: a keychain file to use instead of the default (tests only).
    pub keychain: Option<PathBuf>,
}

impl KeychainStore {
    pub fn from_env() -> Self {
        Self {
            keychain: std::env::var_os("BUNKER_KEYCHAIN")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        }
    }
}

impl KeychainStore {
    /// Exactly one keychain: the login (default) keychain, or the test keychain, unlocked. Never
    /// the whole search list, where an item in some other keychain could shadow bunker's.
    fn keychains(&self) -> Result<Vec<SecKeychain>> {
        let Some(path) = &self.keychain else {
            return Ok(vec![
                SecKeychain::default().map_err(|e| kc_error("open", "the login keychain", e))?,
            ]);
        };
        let mut kc = SecKeychain::open(path)
            .map_err(|e| kc_error("open", &path.display().to_string(), e))?;
        kc.unlock(Some("bunker-test"))
            .map_err(|e| kc_error("unlock", &path.display().to_string(), e))?;
        Ok(vec![kc])
    }
}

impl SecretStore for KeychainStore {
    fn get(&self, project: &str, key: &str) -> Result<Option<Secret>> {
        let kcs = self.keychains()?;
        match find_generic_password(Some(&kcs), &service(project), key) {
            Ok((pw, _)) => Ok(Some(Zeroizing::new(pw.as_ref().to_vec()))),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(kc_error("read", key, e)),
        }
    }
    /// Deletes any existing item and adds a fresh one, so the item always carries the access rule
    /// of its creator, bunker. Updating in place would keep the rule of whoever made it first:
    /// another program could pre-create the item trusting itself and read every later value.
    fn set(&self, project: &str, key: &str, value: &[u8]) -> Result<()> {
        let kc = self.keychains()?.remove(0);
        if let Ok((_, item)) =
            find_generic_password(Some(std::slice::from_ref(&kc)), &service(project), key)
        {
            item.delete();
        }
        kc.add_generic_password(&service(project), key, value)
            .map_err(|e| kc_error("write", key, e))
    }
    fn delete(&self, project: &str, key: &str) -> Result<bool> {
        let kcs = self.keychains()?;
        match find_generic_password(Some(&kcs), &service(project), key) {
            Ok((_, item)) => {
                item.delete();
                Ok(true)
            }
            Err(e) if e.code() == NOT_FOUND => Ok(false),
            Err(e) => Err(kc_error("delete", key, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn sys_keychain_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("bunker-test.keychain-db");
        let ok = std::process::Command::new("/usr/bin/security")
            .args([
                "create-keychain",
                "-p",
                "bunker-test",
                path.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success();
        assert!(ok, "create-keychain failed");
        let store = KeychainStore {
            keychain: Some(path.clone()),
        };
        assert!(store.get("kc", "APP_KEY").unwrap().is_none());
        store.set("kc", "APP_KEY", b"value-one").unwrap();
        store.set("kc", "APP_KEY", b"value-two").unwrap();
        assert_eq!(
            store.get("kc", "APP_KEY").unwrap().unwrap().as_slice(),
            b"value-two"
        );
        assert!(store.delete("kc", "APP_KEY").unwrap());
        assert!(!store.delete("kc", "APP_KEY").unwrap());
        assert!(store.get("kc", "APP_KEY").unwrap().is_none());
        let _ = std::process::Command::new("/usr/bin/security")
            .args(["delete-keychain", path.to_str().unwrap()])
            .status();
    }

    fn acl_apps(path: &std::path::Path) -> String {
        let out = std::process::Command::new("/usr/bin/security")
            .args(["dump-keychain", "-a"])
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    #[ignore]
    fn sys_keychain_set_replaces_a_squatted_item() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("squat.keychain-db");
        let sec = |args: &[&str]| {
            std::process::Command::new("/usr/bin/security")
                .args(args)
                .arg(&path)
                .status()
                .unwrap()
                .success()
        };
        assert!(sec(&["create-keychain", "-p", "bunker-test"]));
        // Another program creates bunker's item first, trusting itself.
        assert!(sec(&[
            "add-generic-password",
            "-s",
            "bunker.kc",
            "-a",
            "APP_KEY",
            "-w",
            "squat",
            "-T",
            "/usr/bin/security"
        ]));
        assert!(
            acl_apps(&path).contains("/usr/bin/security"),
            "control: squatter trusted"
        );
        let store = KeychainStore {
            keychain: Some(path.clone()),
        };
        store.set("kc", "APP_KEY", b"real").unwrap();
        let acl = acl_apps(&path);
        assert!(
            !acl.contains("/usr/bin/security"),
            "the squatter's access survived set:\n{acl}"
        );
        assert_eq!(
            store.get("kc", "APP_KEY").unwrap().unwrap().as_slice(),
            b"real"
        );
        let _ = sec(&["delete-keychain"]);
    }
}
