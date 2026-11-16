//! Renders the guest provisioning script and the files it installs. Every interpolated value comes
//! from a validated `Config` (PHP version, extension and worker names, node major); worker
//! commands are escaped for systemd.
use crate::config::Config;

const PROVISION: &str = include_str!("../../templates/provision.sh");
const NGINX: &str = include_str!("../../templates/nginx-site.conf");
const POOL: &str = include_str!("../../templates/php-fpm-pool.conf");
const WORKER: &str = include_str!("../../templates/worker.service");
const SCHEDULER_SERVICE: &str = include_str!("../../templates/scheduler.service");
const SCHEDULER_TIMER: &str = include_str!("../../templates/scheduler.timer");
const NFTABLES: &str = include_str!("../../templates/nftables.conf");
const LOGROTATE: &str = include_str!("../../templates/logrotate-app");

/// Composer 2.11 needs PHP 7.2.5; older PHP gets the 2.2 LTS line. Checksums from
/// https://getcomposer.org/download/<version>/composer.phar.sha256sum.
const COMPOSER: (&str, &str) = (
    "2.11.2",
    "dd158c0afd41d15fac89e1b4e47b5d42911fe316c3687cd5edac98319b25d028",
);
const COMPOSER_LTS: (&str, &str) = (
    "2.2.32",
    "241a3d13b7c6d35d970b1f69a000298b66943541302148a438a932f5dd44a9fc",
);

/// NodeSource's signing key, fingerprint from https://github.com/nodesource/distributions.
const NODESOURCE_FPR: &str = "6F71F525282841EEDAF851B42F59B5F99B1BE0B4";

/// Replaces each `{{key}}`; a placeholder left over is a bug in bunker, not in the config.
fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    assert!(!out.contains("{{"), "unfilled placeholder in template");
    out
}

/// The file's content without its final newline, for a heredoc body.
fn body(s: &str) -> &str {
    s.strip_suffix('\n').unwrap_or(s)
}

pub fn render_nginx(_cfg: &Config) -> String {
    NGINX.to_string()
}

/// A systemd `ExecStart` string literal: `\` and `"` escaped, `%` and `$` doubled so systemd
/// passes them through to `sh` unchanged.
fn systemd_quote(cmd: &str) -> String {
    cmd.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$")
}

pub fn render_worker(name: &str, cmd: &str) -> String {
    fill(WORKER, &[("name", name), ("cmd", &systemd_quote(cmd))])
}

/// The apt package for a PHP extension: `None` for one compiled into php-common or php-cli.
fn extension_package(php: &str, ext: &str) -> Option<String> {
    const BUILT_IN: &[&str] = &[
        "calendar",
        "ctype",
        "date",
        "exif",
        "ffi",
        "fileinfo",
        "filter",
        "ftp",
        "gettext",
        "hash",
        "iconv",
        "json",
        "libxml",
        "openssl",
        "pcntl",
        "pcre",
        "pdo",
        "phar",
        "posix",
        "reflection",
        "session",
        "shmop",
        "sockets",
        "sodium",
        "spl",
        "standard",
        "sysvmsg",
        "sysvsem",
        "sysvshm",
        "tokenizer",
        "zlib",
    ];
    if BUILT_IN.contains(&ext) {
        return None;
    }
    let pkg = match ext {
        "dom" | "simplexml" | "xmlreader" | "xmlwriter" | "xsl" => "xml",
        "mysqli" | "mysqlnd" | "pdo_mysql" => "mysql",
        "pdo_pgsql" => "pgsql",
        "pdo_sqlite" => "sqlite3",
        other => other,
    };
    Some(format!("php{php}-{pkg}"))
}

fn packages(cfg: &Config) -> String {
    let php = &cfg.php;
    let mut pkgs: Vec<String> = [
        "fpm", "cli", "mysql", "mbstring", "xml", "curl", "zip", "bcmath", "intl",
    ]
    .iter()
    .map(|m| format!("php{php}-{m}"))
    .collect();
    for e in &cfg.extensions {
        if let Some(p) = extension_package(php, e)
            && !pkgs.contains(&p)
        {
            pkgs.push(p);
        }
    }
    for p in [
        "nginx",
        "percona-server-server",
        "nftables",
        "curl",
        "ca-certificates",
        "gnupg",
        "unzip",
        "git",
        "acl",
    ] {
        pkgs.push(p.into());
    }
    if cfg.node.is_some() {
        pkgs.push("nodejs".into());
    }
    pkgs.join(" ")
}

fn node_repo(cfg: &Config) -> String {
    let Some(major) = &cfg.node else {
        return String::new();
    };
    format!(
        "# NodeSource, fingerprint from https://github.com/nodesource/distributions\n\
repo_key nodesource https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key {NODESOURCE_FPR}\n\
put /etc/apt/sources.list.d/nodesource.list 0644 root:root <<'BUNKER_EOF' || true\n\
deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_{major}.x nodistro main\n\
BUNKER_EOF"
    )
}

/// Writes a unit file; a change marks units for daemon-reload and workers for a restart.
fn put_unit(out: &mut String, name: &str, content: &str) {
    out.push_str(&format!(
        "if put /etc/systemd/system/{name} 0644 root:root <<'BUNKER_EOF'; then RELOAD_UNITS=1; RESTART_UNITS=1; fi\n{}\nBUNKER_EOF\n",
        body(content)
    ));
}

fn units(cfg: &Config) -> (String, String) {
    let mut out = String::from("RESTART_UNITS=0\n");
    let mut enable = String::new();
    let keep: Vec<String> = cfg
        .workers
        .keys()
        .map(|n| format!("bunker-worker-{n}.service"))
        .collect();
    for (name, cmd) in &cfg.workers {
        put_unit(
            &mut out,
            &format!("bunker-worker-{name}.service"),
            &render_worker(name, cmd),
        );
        enable.push_str(&format!(
            "systemctl enable --now bunker-worker-{name}.service >/dev/null\n"
        ));
    }
    // Workers removed from the config are stopped and deleted.
    out.push_str("for f in /etc/systemd/system/bunker-worker-*.service; do\n  [ -e \"$f\" ] || continue\n  case ${f##*/} in\n");
    if !keep.is_empty() {
        out.push_str(&format!("  {}) ;;\n", keep.join("|")));
    }
    out.push_str("  *) systemctl disable --now \"${f##*/}\" >/dev/null 2>&1 || true\n     rm -f \"$f\"\n     RELOAD_UNITS=1 ;;\n  esac\ndone\n");
    if cfg.schedule.enabled {
        put_unit(&mut out, "bunker-scheduler.service", SCHEDULER_SERVICE);
        put_unit(&mut out, "bunker-scheduler.timer", SCHEDULER_TIMER);
        enable.push_str("systemctl enable --now bunker-scheduler.timer >/dev/null\n");
    } else {
        out.push_str("if [ -e /etc/systemd/system/bunker-scheduler.timer ]; then\n  systemctl disable --now bunker-scheduler.timer >/dev/null 2>&1 || true\n  rm -f /etc/systemd/system/bunker-scheduler.timer /etc/systemd/system/bunker-scheduler.service\n  RELOAD_UNITS=1\nfi\n");
    }
    enable.push_str(
        "if [ \"$RESTART_UNITS\" = 1 ]; then systemctl try-restart 'bunker-worker-*.service'; fi\n",
    );
    (out, enable)
}

pub fn render_provision(cfg: &Config) -> String {
    let (composer_version, composer_sha256) = if php_before_7_2(&cfg.php) {
        COMPOSER_LTS
    } else {
        COMPOSER
    };
    let (units, enable_units) = units(cfg);
    fill(
        PROVISION,
        &[
            ("php", &cfg.php),
            ("nftables", body(NFTABLES)),
            ("node_repo", &node_repo(cfg)),
            ("packages", &packages(cfg)),
            ("composer_version", composer_version),
            ("composer_sha256", composer_sha256),
            ("pool", body(POOL)),
            ("nginx", body(&render_nginx(cfg))),
            ("units", body(&units)),
            ("logrotate", body(LOGROTATE)),
            ("enable_units", body(&enable_units)),
        ],
    )
}

/// `7.0` and `7.1` (validated versions are `<major>.<minor>`).
fn php_before_7_2(php: &str) -> bool {
    matches!(php, "7.0" | "7.1")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::parse(&crate::config::fixture_config()).unwrap()
    }

    #[test]
    fn nginx_has_hardened_headers_and_real_ip() {
        let n = render_nginx(&cfg());
        for d in [
            "listen 127.0.0.1:80",
            "server_tokens off;",
            "add_header X-Content-Type-Options \"nosniff\" always;",
            "add_header X-Frame-Options \"DENY\" always;",
            "add_header Referrer-Policy \"strict-origin-when-cross-origin\" always;",
            "add_header Permissions-Policy \"camera=(), microphone=(), geolocation=()\" always;",
            "fastcgi_hide_header X-Powered-By;",
            "set_real_ip_from 127.0.0.1;",
            "real_ip_header CF-Connecting-IP;",
            "root /srv/app/current/public;",
            "location ~ /\\.(?!well-known).* {",
        ] {
            assert!(n.contains(d), "missing {d:?} in\n{n}");
        }
    }

    #[test]
    fn worker_unit_runs_as_app_with_restart() {
        let u = render_worker("queue", "php artisan queue:work --tries=3");
        for d in [
            "User=app\n",
            "WorkingDirectory=/srv/app/current\n",
            "Restart=always\n",
            "RestartSec=3\n",
            "ConditionPathExists=/srv/app/current/artisan\n",
            "ExecStart=/bin/sh -c \"php artisan queue:work --tries=3\"\n",
        ] {
            assert!(u.contains(d), "missing {d:?} in\n{u}");
        }
    }

    #[test]
    fn worker_command_is_escaped_for_systemd() {
        let u = render_worker("q", r#"echo "a\b" 50% $HOME"#);
        assert!(
            u.contains(r#"ExecStart=/bin/sh -c "echo \"a\\b\" 50%% $$HOME""#),
            "{u}"
        );
    }

    #[test]
    fn provision_script_is_valid_bash() {
        let s = render_provision(&cfg());
        let mut c = std::process::Command::new("/bin/bash")
            .arg("-n")
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        c.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
        let out = c.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(s.contains("set -euo pipefail"));
        assert!(!s.contains("set -x") && !s.contains("set -o xtrace"));
    }

    #[test]
    fn provision_script_never_puts_password_on_a_command_line() {
        let s = render_provision(&cfg());
        assert!(!s.contains("mysql -e") && !s.contains("--execute"));
        assert!(!s.contains("-p$") && !s.contains("-p\"$") && !s.contains("--password"));
        // The only IDENTIFIED BY lines sit inside the mysql heredoc.
        let start = s.find("<<BUNKER_SQL").expect("mysql heredoc");
        let end = start + s[start..].find("\nBUNKER_SQL\n").expect("heredoc end");
        for (i, _) in s.match_indices("IDENTIFIED BY") {
            assert!(i > start && i < end, "IDENTIFIED BY outside the heredoc");
        }
        assert!(s.contains("IDENTIFIED BY '${BUNKER_DB_PASSWORD}'"));
    }

    #[test]
    fn provision_sets_proxy_for_apt_composer_npm() {
        let s = render_provision(&cfg());
        for d in [
            "http_proxy=http://127.0.0.1:3128",
            "https_proxy=http://127.0.0.1:3128",
            "HTTP_PROXY=http://127.0.0.1:3128",
            "HTTPS_PROXY=http://127.0.0.1:3128",
            "no_proxy=localhost,127.0.0.1",
            "/etc/apt/apt.conf.d/95bunker-proxy",
            "Acquire::https::Proxy \"http://127.0.0.1:3128\";",
            "/home/deploy/.npmrc",
            "https-proxy=http://127.0.0.1:3128",
        ] {
            assert!(s.contains(d), "missing {d:?}");
        }
    }

    #[test]
    fn firewall_keeps_ssh_replies_and_other_tables() {
        let s = render_provision(&cfg());
        // A session open when the rules load is first seen mid-stream as "new"; its replies (from
        // sshd, port 22) must pass or the provisioning session itself is cut.
        let drop = s.find("192.168.0.0/16").unwrap();
        let ssh = s.find("tcp sport 22 accept").expect("ssh replies accepted");
        assert!(ssh < drop);
        // Only bunker's own table is replaced; Lima keeps its nat table.
        assert!(!s.contains("flush ruleset"));
        assert!(s.contains("delete table inet bunker"));
    }

    #[test]
    fn nginx_restarts_when_its_site_changes() {
        // A reload keeps the default site's 0.0.0.0:80 and [::]:80 sockets, which shadow
        // 127.0.0.1:80; only a restart drops them (seen on the first confined provision).
        let s = render_provision(&cfg());
        assert!(s.contains("if [ \"$RESTART_NGINX\" = 1 ]; then systemctl restart nginx; fi"));
        assert!(!s.contains("reload-or-restart nginx"));
    }

    #[test]
    fn root_never_chowns_through_paths_users_control() {
        // app owns shared/storage and deploy owns /home/deploy: anything under them may be a
        // symlink they planted, so root neither chowns nor writes there.
        let s = render_provision(&cfg());
        assert!(
            !s.contains("-o app -g app \"/srv/app/shared/$d\""),
            "root chown under storage"
        );
        assert!(s.contains("runuser -u app -- mkdir -p"));
        assert!(
            !s.contains("put /home/deploy/"),
            "root write into deploy's home"
        );
        assert!(s.contains("runuser -u deploy --"));
        assert!(s.contains("[ -L /srv/app/shared/storage ]"));
    }

    #[test]
    fn provision_turns_off_background_fetches() {
        let s = render_provision(&cfg());
        assert!(
            s.contains("motd-news.timer"),
            "motd-news phones motd.ubuntu.com"
        );
        assert!(s.contains("ENABLED=0"));
    }

    #[test]
    fn repo_keys_are_fingerprint_pinned() {
        let s = render_provision(&cfg());
        for fpr in [
            "14AA40EC0831756756D7F66C4F4EA0AAE5267A6C",
            "4D1BB29D63D98E422B2113B19334A25F8507EFA5",
        ] {
            let call = s
                .lines()
                .find(|l| l.trim_start().starts_with("repo_key ") && l.contains(fpr))
                .unwrap_or_else(|| panic!("no pinned repo_key call for {fpr}"));
            assert!(call.contains("https://"), "{call}");
        }
        // repo_key writes the keyring only after the fingerprint comparison.
        let f = &s[s.find("repo_key() {").unwrap()..];
        let f = &f[..f.find("\n}\n").unwrap()];
        let cmp = f.find("!= \"$3\"").expect("fingerprint comparison");
        let write = f.find("--dearmor").expect("keyring write");
        assert!(cmp < write, "{f}");
    }

    #[test]
    fn node_repo_only_when_node_is_set() {
        let mut c = cfg();
        c.node = None;
        assert!(!render_provision(&c).contains("6F71F525282841EEDAF851B42F59B5F99B1BE0B4"));
        c.node = Some("22".into());
        let s = render_provision(&c);
        assert!(s.contains("6F71F525282841EEDAF851B42F59B5F99B1BE0B4"));
        assert!(s.contains("https://deb.nodesource.com/node_22.x nodistro main"));
    }

    #[test]
    fn packages_follow_php_version_and_extensions() {
        let mut c = cfg();
        c.php = "8.3".into();
        c.extensions = vec![
            "intl".into(),
            "redis".into(),
            "ctype".into(),
            "pdo_mysql".into(),
            "dom".into(),
        ];
        let s = render_provision(&c);
        let line = s.lines().find(|l| l.contains("php8.3-fpm")).unwrap();
        for p in [
            "php8.3-cli",
            "php8.3-mysql",
            "php8.3-intl",
            "php8.3-redis",
            "php8.3-xml",
        ] {
            assert!(line.contains(p), "{p} missing from {line}");
        }
        assert!(
            !line.contains("php8.3-ctype")
                && !line.contains("php8.3-pdo_mysql")
                && !line.contains("php8.3-dom"),
            "{line}"
        );
        assert!(line.contains("percona-server-server"));
        assert!(
            line.split(' ').any(|p| p == "acl"),
            "setfacl for deploy's link_shared"
        );
    }

    #[test]
    fn composer_is_pinned_and_checksummed() {
        let s = render_provision(&cfg());
        assert!(s.contains("https://getcomposer.org/download/2.11.2/composer.phar"));
        assert!(s.contains("dd158c0afd41d15fac89e1b4e47b5d42911fe316c3687cd5edac98319b25d028"));
        let mut c = cfg();
        c.php = "7.1".into();
        assert!(
            render_provision(&c).contains("https://getcomposer.org/download/2.2.32/composer.phar")
        );
    }

    #[test]
    fn workers_and_scheduler_follow_config() {
        let mut c = cfg();
        c.schedule.enabled = true;
        let s = render_provision(&c);
        assert!(s.contains("/etc/systemd/system/bunker-worker-queue.service"));
        assert!(s.contains("/etc/systemd/system/bunker-scheduler.timer"));
        c.schedule.enabled = false;
        c.workers.clear();
        let s = render_provision(&c);
        assert!(!s.contains("/etc/systemd/system/bunker-worker-queue.service"));
        assert!(s.contains("disable --now bunker-scheduler.timer"));
    }

    #[test]
    #[ignore]
    fn sys_shellcheck_provision_script() {
        let mut c = cfg();
        c.node = Some("22".into());
        c.schedule.enabled = true;
        let mut p = std::process::Command::new("shellcheck")
            .args(["-s", "bash", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("shellcheck on PATH");
        use std::io::Write;
        p.stdin
            .take()
            .unwrap()
            .write_all(render_provision(&c).as_bytes())
            .unwrap();
        let out = p.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
