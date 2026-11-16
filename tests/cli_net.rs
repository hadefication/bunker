use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

struct Env {
    home: tempfile::TempDir,
    _repo: tempfile::TempDir,
}

impl Env {
    /// A project "app" created with `bunker init` against a minimal Laravel repo.
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        fs::write(repo.path().join("artisan"), "").unwrap();
        fs::write(
            repo.path().join("composer.json"),
            r#"{"require":{"php":"^8.3","laravel/framework":"^12"}}"#,
        )
        .unwrap();
        let e = Self { home, _repo: repo };
        e.bunker()
            .args(["init", "app", "--path", e._repo.path().to_str().unwrap()])
            .assert()
            .success();
        e
    }

    fn bunker(&self) -> Command {
        let mut c = Command::cargo_bin("bunker").unwrap();
        c.env("BUNKER_HOME", self.home.path())
            .current_dir(self.home.path());
        c
    }

    fn file(&self, name: &str) -> PathBuf {
        self.home.path().join("app").join(name)
    }
}

fn json(out: &[u8]) -> Value {
    serde_json::from_slice(out).unwrap()
}

fn last_line(p: &Path) -> Value {
    let text = fs::read_to_string(p).unwrap();
    serde_json::from_str(text.lines().last().unwrap()).unwrap()
}

#[test]
fn edit_refuses_invalid_document_and_leaves_file() {
    let e = Env::new();
    let cfg = e.file("bunker.toml");
    let broken = fs::read_to_string(&cfg)
        .unwrap()
        .replace("cpus = 2", "cpus = 2\ncpuz = 4");
    fs::write(&cfg, &broken).unwrap();
    let out = e
        .bunker()
        .args(["--json", "net", "allow", "api.example.com", "app"])
        .assert()
        .code(1);
    assert_eq!(
        json(&out.get_output().stdout)["error"]["code"],
        "config_invalid"
    );
    assert_eq!(fs::read_to_string(&cfg).unwrap(), broken);
}

#[test]
fn allow_rejects_internal_ip() {
    let e = Env::new();
    let out = e
        .bunker()
        .args(["--json", "net", "allow", "10.0.0.5", "app"])
        .assert()
        .code(4);
    assert_eq!(
        json(&out.get_output().stdout)["error"]["code"],
        "host_internal"
    );
    e.bunker()
        .args(["net", "allow", "not a host", "app"])
        .assert()
        .code(2);
}

#[test]
fn edit_appends_policy_event() {
    let e = Env::new();
    let out = e
        .bunker()
        .args(["--json", "net", "allow", "api.example.com", "app"])
        .assert()
        .success();
    let v = json(&out.get_output().stdout);
    assert_eq!(v["data"]["project"], "app");
    assert_eq!(v["data"]["changes"][0]["list"], "allow");
    assert_eq!(v["data"]["changes"][0]["after"][0], "api.example.com");
    let ev = last_line(&e.file("network.jsonl"));
    assert_eq!(
        (ev["kind"].as_str(), ev["source"].as_str()),
        (Some("policy"), Some("edit"))
    );
    assert_eq!(ev["allow"][0], "api.example.com");

    // Human output shows the diff; a repeat changes nothing and says so.
    let out = e
        .bunker()
        .current_dir(e._repo.path())
        .args(["net", "allow", "--build", "pkg.example.com"])
        .assert()
        .success();
    assert!(String::from_utf8_lossy(&out.get_output().stdout).contains("+ pkg.example.com"));
    let out = e
        .bunker()
        .args(["net", "allow", "api.example.com", "app"])
        .assert()
        .success();
    assert!(String::from_utf8_lossy(&out.get_output().stdout).contains("already allowed"));
    let lines = fs::read_to_string(e.file("network.jsonl"))
        .unwrap()
        .lines()
        .count();
    assert_eq!(lines, 2, "a no-op edit logs nothing");

    let out = e
        .bunker()
        .args(["net", "deny", "api.example.com", "app"])
        .assert()
        .success();
    let text = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    assert!(
        text.contains("- api.example.com") && text.contains("+ api.example.com"),
        "{text}"
    );
    let out = e
        .bunker()
        .args(["net", "rm", "nothing.example.com", "app"])
        .assert()
        .success();
    assert!(String::from_utf8_lossy(&out.get_output().stdout).contains("not listed"));
}

#[test]
fn reload_validates_diffs_and_logs() {
    let e = Env::new();
    let cfg = e.file("bunker.toml");
    let text = fs::read_to_string(&cfg)
        .unwrap()
        .replace("allow = []", "allow = [\"hand.example.com\"]");
    fs::write(&cfg, text).unwrap();
    let out = e.bunker().args(["net", "reload", "app"]).assert().success();
    assert!(String::from_utf8_lossy(&out.get_output().stdout).contains("+ hand.example.com"));
    assert_eq!(last_line(&e.file("network.jsonl"))["source"], "reload");

    let text = fs::read_to_string(&cfg)
        .unwrap()
        .replace("allow = [", "allow = [\"10.1.1.1\", ");
    fs::write(&cfg, text).unwrap();
    e.bunker().args(["net", "reload", "app"]).assert().code(1);
    assert_eq!(last_line(&e.file("network.jsonl"))["kind"], "policy_error");
}

#[test]
fn net_log_blocked_filters_decisions() {
    let e = Env::new();
    let log = e.file("network.jsonl");
    let mut lines = String::new();
    for (host, verdict) in [
        ("a.example", "allow"),
        ("b.example", "block"),
        ("c.example", "block"),
    ] {
        lines += &format!(
            "{{\"ts\":\"2026-11-16T00:00:00Z\",\"project\":\"app\",\"kind\":\"decision\",\"host\":\"{host}\",\"port\":443,\"phase\":\"run\",\"verdict\":\"{verdict}\",\"reason\":\"not_listed\"}}\n"
        );
    }
    lines += "not json\n";
    fs::write(&log, lines).unwrap();
    let out = e
        .bunker()
        .args(["net", "log", "app", "--blocked"])
        .assert()
        .success();
    let text = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    assert!(
        !text.contains("a.example") && text.contains("b.example") && text.contains("c.example"),
        "{text}"
    );
    let out = e
        .bunker()
        .args(["--json", "net", "log", "app", "-n", "1"])
        .assert()
        .success();
    let v = json(&out.get_output().stdout);
    assert_eq!(v["data"]["events"].as_array().unwrap().len(), 1);
    assert_eq!(v["data"]["events"][0]["host"], "c.example");
}
