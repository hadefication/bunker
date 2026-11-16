//! Generates in-app secrets that don't exist yet: APP_KEY, DB_PASSWORD, and Passport's key pair.
use crate::config::{Config, SecretKind};
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use crate::paths::ProjectDir;
use crate::run::{Cmd, Runner};
use crate::secrets::meta::{Entry, record};
use crate::secrets::store::SecretStore;
use zeroize::Zeroizing;

/// Creates only keys absent from the store, records them in `[secrets]`, returns their names.
pub fn generate_missing(
    pd: &ProjectDir,
    cfg: &Config,
    detected_passport: bool,
    store: &dyn SecretStore,
    r: &dyn Runner,
) -> Result<Vec<String>> {
    let mut made = Vec::new();
    let have = |k: &str| -> Result<bool> { Ok(store.get(&pd.name, k)?.is_some()) };
    let mut values: Vec<(&str, Zeroizing<Vec<u8>>)> = Vec::new();
    if !have("APP_KEY")? {
        values.push((
            "APP_KEY",
            Zeroizing::new(format!("base64:{}", base64(&random_bytes(32))).into_bytes()),
        ));
    }
    if !have("DB_PASSWORD")? {
        const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        // 256 % 62 != 0, so reject bytes past the last full multiple to keep it uniform.
        let mut pw = Zeroizing::new(Vec::with_capacity(32));
        while pw.len() < 32 {
            for b in random_bytes(64) {
                if b < 248 && pw.len() < 32 {
                    pw.push(ALNUM[(b % 62) as usize]);
                }
            }
        }
        values.push(("DB_PASSWORD", pw));
    }
    if detected_passport && (!have("PASSPORT_PRIVATE_KEY")? || !have("PASSPORT_PUBLIC_KEY")?) {
        let private = Zeroizing::new(
            r.run_ok(&Cmd::new("/usr/bin/openssl").args([
                "genpkey",
                "-algorithm",
                "RSA",
                "-pkeyopt",
                "rsa_keygen_bits:4096",
            ]))?
            .stdout,
        );
        let public = r
            .run_ok(
                &Cmd::new("/usr/bin/openssl")
                    .args(["pkey", "-pubout"])
                    .stdin(private.to_vec()),
            )?
            .stdout;
        if !private.starts_with(b"-----BEGIN") || !public.starts_with(b"-----BEGIN PUBLIC KEY") {
            return Err(BunkerError::new(
                "keygen_failed",
                ExitCode::Failure,
                "openssl did not return a PEM key pair",
            ));
        }
        values.push(("PASSPORT_PRIVATE_KEY", private));
        values.push(("PASSPORT_PUBLIC_KEY", Zeroizing::new(public)));
    }
    for (k, v) in &values {
        store.set(&pd.name, k, v)?;
        made.push(k.to_string());
    }
    let mut names: Vec<&str> = vec!["APP_KEY", "DB_PASSWORD"];
    if detected_passport {
        names.extend(["PASSPORT_PRIVATE_KEY", "PASSPORT_PUBLIC_KEY"]);
    }
    for k in names {
        if !cfg.secrets.contains_key(k) {
            record(pd, k, Entry::Secret(SecretKind::InApp))?;
        }
    }
    Ok(made)
}

/// Cryptographically random bytes from the kernel (arc4random_buf).
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    // SAFETY: arc4random_buf fills exactly n bytes of the buffer we own.
    unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), n) };
    b
}

/// Standard base64 with padding (RFC 4648).
pub fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for chunk in b.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &x)| acc | (u32::from(x) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{Cmd, Output, RecordingRunner};
    use crate::secrets::store::MemoryStore;

    const PEM: &str =
        "-----BEGIN PRIVATE KEY-----\nMIIfixture-private\n-----END PRIVATE KEY-----\n";

    fn decode(s: &str) -> Vec<u8> {
        let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut bits = 0u32;
        let mut n = 0;
        let mut out = vec![];
        for c in s.bytes().filter(|c| *c != b'=') {
            bits = (bits << 6) | table.iter().position(|t| *t == c).unwrap() as u32;
            n += 6;
            if n >= 8 {
                n -= 8;
                out.push((bits >> n) as u8);
            }
        }
        out
    }

    fn setup() -> (tempfile::TempDir, ProjectDir, Config) {
        let home = tempfile::tempdir().unwrap();
        let pd = ProjectDir::under(home.path(), "p");
        std::fs::create_dir_all(&pd.root).unwrap();
        std::fs::write(pd.config(), crate::config::fixture_config()).unwrap();
        let cfg = Config::load(&pd).unwrap();
        (home, pd, cfg)
    }

    fn openssl() -> RecordingRunner {
        let r = RecordingRunner::new();
        r.respond("/usr/bin/openssl", &["genpkey"], Output::ok(PEM));
        r.respond_with("/usr/bin/openssl", &["pkey", "-pubout"], |c: &Cmd| {
            assert_eq!(c.stdin.as_deref(), Some(PEM.as_bytes()));
            Output::ok("-----BEGIN PUBLIC KEY-----\nMIIfixture-public\n-----END PUBLIC KEY-----\n")
        });
        r
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn app_key_format() {
        let (_h, pd, cfg) = setup();
        let store = MemoryStore::default();
        generate_missing(&pd, &cfg, false, &store, &RecordingRunner::new()).unwrap();
        let key = String::from_utf8(store.get("p", "APP_KEY").unwrap().unwrap().to_vec()).unwrap();
        let b = key.strip_prefix("base64:").expect(&key);
        assert_eq!(decode(b).len(), 32);
        let pw =
            String::from_utf8(store.get("p", "DB_PASSWORD").unwrap().unwrap().to_vec()).unwrap();
        assert!(
            pw.len() == 32 && pw.chars().all(|c| c.is_ascii_alphanumeric()),
            "{pw}"
        );
    }

    #[test]
    fn generate_creates_only_missing() {
        let (_h, pd, cfg) = setup();
        let store = MemoryStore::default();
        store.set("p", "APP_KEY", b"base64:existing").unwrap();
        let made = generate_missing(&pd, &cfg, false, &store, &RecordingRunner::new()).unwrap();
        assert_eq!(made, ["DB_PASSWORD"]);
        assert_eq!(
            store.get("p", "APP_KEY").unwrap().unwrap().as_slice(),
            b"base64:existing"
        );
        let cfg = Config::load(&pd).unwrap();
        assert!(
            cfg.secrets.contains_key("APP_KEY") && cfg.secrets.contains_key("DB_PASSWORD"),
            "both recorded"
        );
        assert!(
            generate_missing(&pd, &cfg, false, &store, &RecordingRunner::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn openssl_private_key_never_in_argv() {
        let (_h, pd, cfg) = setup();
        let store = MemoryStore::default();
        let r = openssl();
        let made = generate_missing(&pd, &cfg, true, &store, &r).unwrap();
        assert!(
            made.contains(&"PASSPORT_PRIVATE_KEY".to_string())
                && made.contains(&"PASSPORT_PUBLIC_KEY".to_string())
        );
        r.assert_no_secret_in_argv(&["MIIfixture-private"]);
        let calls = r.calls();
        assert_eq!(
            calls[0].args,
            [
                "genpkey",
                "-algorithm",
                "RSA",
                "-pkeyopt",
                "rsa_keygen_bits:4096"
            ]
        );
        assert_eq!(
            store
                .get("p", "PASSPORT_PRIVATE_KEY")
                .unwrap()
                .unwrap()
                .as_slice(),
            PEM.as_bytes()
        );
    }
}
