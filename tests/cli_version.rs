use assert_cmd::Command;

/// `bunker 2.0.0-alpha.0 (<sha>[-dirty], YYYY-MM-DD)\n`, checked without a regex crate.
fn assert_version_line(out: &str) {
    let rest = out
        .strip_prefix("bunker 2.0.0-alpha.0 (")
        .unwrap_or_else(|| panic!("bad prefix: {out:?}"));
    let rest = rest
        .strip_suffix(")\n")
        .unwrap_or_else(|| panic!("bad suffix: {out:?}"));
    let (commit, date) = rest
        .split_once(", ")
        .unwrap_or_else(|| panic!("no comma: {out:?}"));
    let sha = commit.strip_suffix("-dirty").unwrap_or(commit);
    assert!(
        sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()),
        "bad sha: {commit:?}"
    );
    let parts: Vec<&str> = date.split('-').collect();
    assert_eq!(
        parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
        [4, 2, 2],
        "bad date: {date:?}"
    );
    assert!(
        date.chars().all(|c| c.is_ascii_digit() || c == '-'),
        "bad date: {date:?}"
    );
}

#[test]
fn version_flag_prints_version_commit_and_date() {
    for args in [&["--version"][..], &["-V"], &["version"]] {
        let out = Command::cargo_bin("bunker")
            .unwrap()
            .args(args)
            .assert()
            .success()
            .get_output()
            .clone();
        assert_version_line(&String::from_utf8(out.stdout).unwrap());
    }
}
