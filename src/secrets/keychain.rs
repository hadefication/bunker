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
    /// `None` searches the default list; `Some` is the one test keychain, unlocked.
    fn keychains(&self) -> Result<Option<Vec<SecKeychain>>> {
        let Some(path) = &self.keychain else {
            return Ok(None);
        };
        let mut kc = SecKeychain::open(path)
            .map_err(|e| kc_error("open", &path.display().to_string(), e))?;
        kc.unlock(Some("bunker-test"))
            .map_err(|e| kc_error("unlock", &path.display().to_string(), e))?;
        Ok(Some(vec![kc]))
    }
}

impl SecretStore for KeychainStore {
    fn get(&self, project: &str, key: &str) -> Result<Option<Secret>> {
        let kcs = self.keychains()?;
        match find_generic_password(kcs.as_deref(), &service(project), key) {
            Ok((pw, _)) => Ok(Some(Zeroizing::new(pw.as_ref().to_vec()))),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(kc_error("read", key, e)),
        }
    }
    fn set(&self, project: &str, key: &str, value: &[u8]) -> Result<()> {
        let kc = match &self.keychain {
            Some(_) => self.keychains()?.unwrap().remove(0),
            None => SecKeychain::default().map_err(|e| kc_error("open", key, e))?,
        };
        kc.set_generic_password(&service(project), key, value)
            .map_err(|e| kc_error("write", key, e))
    }
    fn delete(&self, project: &str, key: &str) -> Result<bool> {
        let kcs = self.keychains()?;
        match find_generic_password(kcs.as_deref(), &service(project), key) {
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
}
