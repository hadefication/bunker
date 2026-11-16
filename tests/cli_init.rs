use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn laravel_repo(dir: &Path) {
    fs::write(dir.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    fs::write(
        dir.join("composer.json"),
        r#"{"require":{"php":"^8.3","laravel/framework":"^12.0","laravel/passport":"^13.0","ext-intl":"*"}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("package.json"),
        r#"{"scripts":{"build":"vite build"}}"#,
    )
    .unwrap();
}

fn bunker(home: &Path) -> Command {
    let mut c = Command::cargo_bin("bunker").unwrap();
    c.env("BUNKER_HOME", home);
    c
}

#[test]
fn init_writes_0600_config_and_refuses_overwrite() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    laravel_repo(repo.path());
    let path = repo.path().to_str().unwrap();
    bunker(home.path())
        .args([
            "init",
            "life-os",
            "--path",
            path,
            "--domain",
            "life-os.example.com",
        ])
        .assert()
        .success();
    let cfg = home.path().join("life-os/bunker.toml");
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&cfg), 0o600);
    assert_eq!(mode(&home.path().join("life-os")), 0o700);
    let text = fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("tunnel = \"life-os\""), "{text}");
    assert!(text.contains('#'), "the config carries comments: {text}");

    let out = bunker(home.path())
        .args(["init", "life-os", "--path", path])
        .assert()
        .code(4);
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).into_owned();
    assert!(stderr.contains("--force"), "{stderr}");
    assert_eq!(
        fs::read_to_string(&cfg).unwrap(),
        text,
        "refused init left the file alone"
    );

    bunker(home.path())
        .args(["init", "life-os", "--path", path, "--php", "8.3", "--force"])
        .assert()
        .success();
    assert!(fs::read_to_string(&cfg).unwrap().contains("php = \"8.3\""));
}

#[test]
fn init_json_reports_detected_values() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    laravel_repo(repo.path());
    let out = bunker(home.path())
        .args([
            "--json",
            "init",
            "app",
            "--path",
            repo.path().to_str().unwrap(),
        ])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["ok"], true);
    let d = &v["data"];
    assert_eq!(d["php"], "8.5");
    assert_eq!(d["stack"], "laravel");
    assert_eq!(d["passport"], true);
    assert!(d["domain"].is_null() && d["tunnel"].is_null());
    let ssh = d["ports"]["ssh"].as_u64().unwrap();
    assert!((60100..=60199).contains(&ssh), "{ssh}");
    assert!((18800..=18899).contains(&d["ports"]["proxy"].as_u64().unwrap()));
    assert!((18900..=18999).contains(&d["ports"]["web"].as_u64().unwrap()));
}

#[test]
fn init_refuses_bad_input() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    laravel_repo(repo.path());
    let path = repo.path().to_str().unwrap();
    let out = bunker(home.path())
        .args(["--json", "init", "Bad Name", "--path", path])
        .assert()
        .code(2);
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["error"]["code"], "invalid_value");
    bunker(home.path())
        .args(["init", "app", "--path", path, "--php", "9.9"])
        .assert()
        .code(2);
    let empty = tempfile::tempdir().unwrap();
    bunker(home.path())
        .args(["init", "app", "--path", empty.path().to_str().unwrap()])
        .assert()
        .code(4);
    assert!(!home.path().join("app").exists());
}
