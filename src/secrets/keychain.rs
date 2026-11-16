//! macOS Keychain generic passwords, service `bunker.<project>`, account = the env key. Items are
//! created by bunker, so the legacy keychain's default access lets bunker read them without a
//! prompt and makes every other process (an agent's `security find-generic-password`) ask.
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use crate::secrets::store::{Secret, SecretStore};
use security_framework::item::{ItemClass, ItemSearchOptions, Reference, SearchResult};
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::os::macos::keychain_item::SecKeychainItem;
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
    /// Exactly one keychain: the login keychain opened by its path, or the test keychain. Never
    /// the default keychain (anything running as the user can repoint it at a keychain it
    /// controls) and never the whole search list (an item elsewhere could shadow bunker's).
    fn keychains(&self) -> Result<Vec<SecKeychain>> {
        let Some(path) = &self.keychain else {
            let login = login_keychain_path(&user_home()?);
            let kc = SecKeychain::open(&login)
                .map_err(|e| kc_error("open", &login.display().to_string(), e))?;
            return Ok(vec![kc]);
        };
        let mut kc = SecKeychain::open(path)
            .map_err(|e| kc_error("open", &path.display().to_string(), e))?;
        kc.unlock(Some("bunker-test"))
            .map_err(|e| kc_error("unlock", &path.display().to_string(), e))?;
        Ok(vec![kc])
    }
}

/// The user's home from the password database, not `$HOME`: an empty or relative `$HOME` would
/// make the keychain path resolve against the current directory.
pub fn user_home() -> Result<PathBuf> {
    let fail = || {
        BunkerError::new(
            "home_dir",
            ExitCode::Failure,
            "cannot find this user's home directory",
        )
    };
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    let mut buf = vec![0 as libc::c_char; 4096];
    // SAFETY: getpwuid_r writes into pw and buf (sized by buf.len()) and sets out on success.
    let rc = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut pw,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        )
    };
    if rc != 0 || out.is_null() || pw.pw_dir.is_null() {
        return Err(fail());
    }
    // SAFETY: pw_dir points into buf, NUL-terminated by getpwuid_r.
    let dir = unsafe { std::ffi::CStr::from_ptr(pw.pw_dir) };
    let home = PathBuf::from(
        <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(dir.to_bytes()),
    );
    if !home.is_absolute() {
        return Err(fail());
    }
    Ok(home)
}

/// `~/Library/Keychains/login.keychain-db`.
pub fn login_keychain_path(home: &std::path::Path) -> PathBuf {
    home.join("Library/Keychains/login.keychain-db")
}

/// The item for `key`, found without reading its value. Reading asks for approval when the item
/// does not trust bunker (one another program planted), and deleting it needs no value.
fn find_item(kcs: &[SecKeychain], project: &str, key: &str) -> Result<Option<SecKeychainItem>> {
    let found = ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .keychains(kcs)
        .service(&service(project))
        .account(key)
        .load_refs(true)
        .limit(1)
        .search();
    match found {
        Ok(results) => Ok(results.into_iter().find_map(|r| match r {
            SearchResult::Ref(Reference::KeychainItem(item)) => Some(item),
            _ => None,
        })),
        Err(e) if e.code() == NOT_FOUND => Ok(None),
        Err(e) => Err(kc_error("find", key, e)),
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
        if let Some(item) = find_item(std::slice::from_ref(&kc), project, key)? {
            item.delete();
        }
        kc.add_generic_password(&service(project), key, value)
            .map_err(|e| kc_error("write", key, e))
    }
    fn delete(&self, project: &str, key: &str) -> Result<bool> {
        let kcs = self.keychains()?;
        match find_item(&kcs, project, key)? {
            Some(item) => {
                item.delete();
                Ok(true)
            }
            None => Ok(false),
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
        // A prompt fails the call instead of showing a dialog: replacing a squatted item must not
        // need the squatter's value (reading it is what asks for approval).
        let _no_prompts = SecKeychain::disable_user_interaction().unwrap();
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

    #[test]
    fn login_keychain_is_opened_by_path_not_by_default() {
        assert_eq!(
            login_keychain_path(std::path::Path::new("/Users/x")),
            std::path::PathBuf::from("/Users/x/Library/Keychains/login.keychain-db")
        );
    }

    #[test]
    #[ignore]
    fn sys_login_keychain_opens_without_prompt() {
        let store = KeychainStore { keychain: None };
        assert!(
            store
                .get("bunker-probe-nonexistent", "NOPE")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn user_home_comes_from_the_password_database() {
        let h = user_home().unwrap();
        assert!(h.is_absolute() && h.is_dir(), "{}", h.display());
    }
}
