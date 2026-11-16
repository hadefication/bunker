//! Redacts secrets from log lines before bunker prints them: the project's own Keychain values,
//! Authorization and Bearer credentials, password/secret/token/api-key values, and private keys.
use std::borrow::Cow;
use std::cell::Cell;
use zeroize::Zeroizing;

const REDACTED: &str = "[redacted]";
/// A `k=v` or `"k":"v"` value is redacted when the key, lowercased, ends with one of these.
const SECRET_KEYS: [&str; 7] = [
    "password", "passwd", "secret", "token", "api_key", "api-key", "apikey",
];
/// Values shorter than this would redact ordinary words.
const MIN_KNOWN: usize = 6;

pub struct Scrubber {
    known: Vec<(String, Zeroizing<String>)>,
    /// Inside a `-----BEGIN ... PRIVATE KEY-----` block, across lines.
    in_pem: Cell<bool>,
}

impl Scrubber {
    pub fn new(known: Vec<(String, Zeroizing<String>)>) -> Scrubber {
        let mut known: Vec<_> = known
            .into_iter()
            .filter(|(_, v)| v.len() >= MIN_KNOWN)
            .collect();
        // Longest first, so a value containing another is redacted whole.
        known.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
        Scrubber {
            known,
            in_pem: Cell::new(false),
        }
    }

    /// The line with secrets replaced; borrowed when nothing changed.
    pub fn scrub<'a>(&self, line: &'a str) -> Cow<'a, str> {
        if self.in_pem.get() {
            if line.contains("-----END") {
                self.in_pem.set(false);
            }
            return Cow::Owned(REDACTED.into());
        }
        if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
            self.in_pem.set(!line.contains("-----END"));
            return Cow::Owned(REDACTED.into());
        }
        let mut cur = Cow::Borrowed(line);
        for (key, value) in &self.known {
            if cur.contains(value.as_str()) {
                cur = Cow::Owned(cur.replace(value.as_str(), &format!("[redacted:{key}]")));
            }
        }
        if let Some(i) = find_ci(&cur, "authorization:") {
            let end = i + "authorization:".len();
            if cur[end..].trim() != REDACTED {
                cur = Cow::Owned(format!("{} {REDACTED}", &cur[..end]));
            }
        }
        if let Some(out) = redact_bearer(&cur) {
            cur = Cow::Owned(out);
        }
        if let Some(out) = redact_assignments(&cur) {
            cur = Cow::Owned(out);
        }
        if let Some(out) = redact_json(&cur) {
            cur = Cow::Owned(out);
        }
        cur
    }
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    hay.to_ascii_lowercase().find(needle)
}

fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    SECRET_KEYS.iter().any(|w| k.ends_with(w))
}

/// Replaces `text[a..b]` with `[redacted]` unless it is already redacted or empty.
fn splice(out: &mut String, a: usize, b: usize) -> bool {
    if a >= b || out[a..b].starts_with("[redacted") {
        return false;
    }
    out.replace_range(a..b, REDACTED);
    true
}

/// The token after `Bearer ` (any case).
fn redact_bearer(line: &str) -> Option<String> {
    let mut out = line.to_string();
    let mut changed = false;
    let mut from = 0;
    while let Some(i) = out[from..]
        .to_ascii_lowercase()
        .find("bearer ")
        .map(|i| i + from)
    {
        let a = i + "bearer ".len();
        let b = out[a..]
            .find(char::is_whitespace)
            .map_or(out.len(), |e| a + e);
        changed |= splice(&mut out, a, b);
        from = (a + REDACTED.len()).min(out.len());
    }
    changed.then_some(out)
}

/// `key=value` where the key names a secret; the value runs to whitespace or a separator.
fn redact_assignments(line: &str) -> Option<String> {
    let mut out = line.to_string();
    let mut changed = false;
    let mut from = 0;
    while let Some(eq) = out[from..].find('=').map(|i| i + from) {
        let key_start = out[..eq]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || "_-.".contains(c)))
            .map_or(0, |i| i + 1);
        let a = eq + 1;
        let b = out[a..]
            .find(|c: char| c.is_whitespace() || "&,;\"')]}".contains(c))
            .map_or(out.len(), |e| a + e);
        if is_secret_key(&out[key_start..eq]) && splice(&mut out, a, b) {
            changed = true;
            from = a + REDACTED.len();
        } else {
            from = a;
        }
        if from >= out.len() {
            break;
        }
    }
    changed.then_some(out)
}

/// The end of the JSON string starting at `start` (just past its opening quote): the index of
/// its closing quote, skipping escaped ones.
fn string_end(s: &str, start: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = start;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// `"key": "value"` where the key names a secret.
fn redact_json(line: &str) -> Option<String> {
    let mut out = line.to_string();
    let mut changed = false;
    let mut i = 0;
    while let Some(q) = out[i..].find('"').map(|x| x + i) {
        let Some(kend) = string_end(&out, q + 1) else {
            break;
        };
        let key = out[q + 1..kend].to_string();
        let rest = &out[kend + 1..];
        let after_colon = rest
            .trim_start()
            .strip_prefix(':')
            .map(|r| r.trim_start())
            .filter(|r| r.starts_with('"'));
        let Some(r) = after_colon else {
            i = kend + 1;
            continue;
        };
        let vstart = out.len() - r.len() + 1;
        let Some(vend) = string_end(&out, vstart) else {
            break;
        };
        if is_secret_key(&key) && splice(&mut out, vstart, vend) {
            changed = true;
            i = vstart + REDACTED.len() + 1;
        } else {
            i = vend + 1;
        }
    }
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(known: &[(&str, &str)]) -> Scrubber {
        Scrubber::new(
            known
                .iter()
                .map(|(k, v)| (k.to_string(), Zeroizing::new(v.to_string())))
                .collect(),
        )
    }

    #[test]
    fn scrubber_redacts_known_values() {
        let sc = s(&[("DB_PASSWORD", "Zx81LqTrw0"), ("SHORT", "abc12")]);
        assert_eq!(
            sc.scrub("connect failed for app:Zx81LqTrw0@127.0.0.1"),
            "connect failed for app:[redacted:DB_PASSWORD]@127.0.0.1"
        );
        // Values under 6 characters would redact ordinary words; they're left alone.
        assert_eq!(sc.scrub("abc12 rows"), "abc12 rows");
    }

    #[test]
    fn scrubber_redacts_patterns_and_pem_blocks() {
        let sc = s(&[]);
        assert_eq!(
            sc.scrub("> Authorization: Basic dXNlcjpwYXNz"),
            "> Authorization: [redacted]"
        );
        assert_eq!(
            sc.scrub("header bearer eyJhbGciOi.x.y sent"),
            "header bearer [redacted] sent"
        );
        assert_eq!(
            sc.scrub("login password=hunter22 user=bob"),
            "login password=[redacted] user=bob"
        );
        assert_eq!(
            sc.scrub("MAIL_API_KEY=sk_live_abc&x=1"),
            "MAIL_API_KEY=[redacted]&x=1"
        );
        assert_eq!(
            sc.scrub(r#"{"client_secret":"s3cr3t-value","id":7}"#),
            r#"{"client_secret":"[redacted]","id":7}"#
        );
        assert_eq!(
            sc.scrub(r#"{"token": "abc.def"}"#),
            r#"{"token": "[redacted]"}"#
        );
        assert_eq!(sc.scrub("-----BEGIN RSA PRIVATE KEY-----"), "[redacted]");
        assert_eq!(sc.scrub("MIIEpAIBAAKCAQEA"), "[redacted]");
        assert_eq!(sc.scrub("-----END RSA PRIVATE KEY-----"), "[redacted]");
        assert_eq!(sc.scrub("after the key"), "after the key");
    }

    #[test]
    fn scrubber_keeps_ordinary_lines() {
        let sc = s(&[("APP_KEY", "base64:abcdefghij")]);
        for line in [
            "GET /up HTTP/1.1 200",
            "[2026-11-16 03:04:05] production.INFO: queue worker started",
            "tokens_used=5 is not a token= assignment? it is: no, key is tokens_used",
        ] {
            assert!(matches!(sc.scrub(line), Cow::Borrowed(_)), "{line}");
        }
    }
}
