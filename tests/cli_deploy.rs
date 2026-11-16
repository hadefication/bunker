use assert_cmd::Command;

fn bunker(home: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("bunker").unwrap();
    c.env("BUNKER_HOME", home);
    c
}

#[test]
fn deploy_and_rollback_of_unknown_project_are_not_found() {
    let home = tempfile::tempdir().unwrap();
    for args in [
        &["--json", "deploy", "nope"][..],
        &["--json", "rollback", "nope"],
    ] {
        let out = bunker(home.path()).args(args).assert().code(3);
        let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
        assert_eq!(v["error"]["code"], "project_not_found", "{args:?}");
    }
}

#[test]
fn deploy_help_lists_its_flags() {
    let home = tempfile::tempdir().unwrap();
    let out = bunker(home.path())
        .args(["deploy", "--help"])
        .assert()
        .success();
    let text = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    assert!(
        text.contains("--dirty") && text.contains("--allow-advisories"),
        "{text}"
    );
}
