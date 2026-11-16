use assert_cmd::Command;
use std::fs;

fn bunker(home: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("bunker").unwrap();
    c.env("BUNKER_HOME", home);
    c
}

fn init(home: &std::path::Path) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    fs::write(repo.path().join("artisan"), "").unwrap();
    fs::write(
        repo.path().join("composer.json"),
        r#"{"require":{"php":"^8.3","laravel/framework":"^12"}}"#,
    )
    .unwrap();
    bunker(home)
        .args(["init", "t", "--path", repo.path().to_str().unwrap()])
        .assert()
        .success();
    repo
}

#[test]
fn status_with_no_projects_is_an_empty_array() {
    let home = tempfile::tempdir().unwrap();
    let out = bunker(home.path())
        .args(["--json", "status"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["data"], serde_json::json!([]));
}

#[test]
fn unknown_project_is_not_found_for_every_command() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["--json", "status", "nope"][..],
        &["--json", "logs", "nope", "app"],
        &["--json", "artisan", "nope", "--", "about"],
        &["--json", "shell", "nope"],
    ] {
        let out = bunker(home.path()).args(args).assert().code(3);
        let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
        assert_eq!(v["error"]["code"], "project_not_found", "{args:?}");
    }
}

#[test]
fn shell_without_a_terminal_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let _repo = init(home.path());
    let out = bunker(home.path())
        .args(["--json", "shell", "t"])
        .assert()
        .code(4);
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["error"]["code"], "needs_tty");
}

#[test]
fn logs_rejects_an_unknown_source() {
    let home = tempfile::tempdir().unwrap();
    let _repo = init(home.path());
    let out = bunker(home.path())
        .args(["--json", "logs", "t", "bogus"])
        .assert()
        .code(2);
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["error"]["code"], "invalid_value");
}
