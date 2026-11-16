//! Refusals that happen before any Keychain or VM access.
use assert_cmd::Command;
use serde_json::Value;

fn refused(args: &[&str], code: &str) {
    let home = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("bunker")
        .unwrap()
        .env("BUNKER_HOME", home.path())
        .arg("--json")
        .args(args)
        .assert()
        .code(4);
    let v: Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["error"]["code"], code, "{args:?}");
}

#[test]
fn plain_cannot_override_protected_keys() {
    refused(&["env", "set", "APP_KEY", "x", "--plain"], "key_protected");
    refused(
        &["env", "set", "DB_HOST", "x", "t", "--plain"],
        "key_protected",
    );
}

#[test]
fn env_set_bind_is_refused_for_now() {
    refused(
        &["env", "set", "MAIL_KEY", "t", "--bind", "api.mailgun.net"],
        "not_yet_supported",
    );
}

#[test]
fn env_set_without_tty_or_stdin_needs_tty() {
    refused(&["env", "set", "API_TOKEN", "t"], "needs_tty");
}

#[test]
fn env_set_empty_stdin_is_refused() {
    let home = tempfile::tempdir().unwrap();
    Command::cargo_bin("bunker")
        .unwrap()
        .env("BUNKER_HOME", home.path())
        .args(["env", "set", "API_TOKEN", "t", "--stdin"])
        .write_stdin("")
        .assert()
        .code(4);
}
