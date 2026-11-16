use assert_cmd::Command;
use std::fs;

#[test]
fn destroy_without_yes_needs_tty() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    fs::write(repo.path().join("artisan"), "").unwrap();
    fs::write(
        repo.path().join("composer.json"),
        r#"{"require":{"php":"^8.3","laravel/framework":"^12"}}"#,
    )
    .unwrap();
    let bunker = || {
        let mut c = Command::cargo_bin("bunker").unwrap();
        c.env("BUNKER_HOME", home.path());
        c
    };
    bunker()
        .args(["init", "t", "--path", repo.path().to_str().unwrap()])
        .assert()
        .success();
    let out = bunker().args(["--json", "destroy", "t"]).assert().code(4);
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["error"]["code"], "needs_tty");
    assert!(v["error"]["hint"].as_str().unwrap().contains("--yes"));
    assert!(home.path().join("t/bunker.toml").exists());
}
