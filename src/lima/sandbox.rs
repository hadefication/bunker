//! Seatbelt profiles. They restrict networking only (see the spec's threat model); each starts
//! from `(allow default)`. Ports are `localhost:<port>`, which covers 127.0.0.1 and ::1.
use std::path::Path;

/// The hostagent's profile: the spike's network rules, plus (plan 2 hardening) writes only under
/// the VM dir, Lima's `_config`, the per-user temp dir and terminal devices, and exec of `limactl`
/// and `/usr/bin/ssh` only. Network rules are never widened here.
pub fn hostagent_profile(
    lima_vm_dir_real: &Path,
    ssh_port: u16,
    limactl: &Path,
    user_tmp: &Path,
) -> String {
    let dir = sb_string(&lima_vm_dir_real.to_string_lossy());
    let config = sb_string(
        &lima_vm_dir_real
            .parent()
            .unwrap_or(Path::new("/nonexistent"))
            .join("_config")
            .to_string_lossy(),
    );
    let tmp = sb_string(user_tmp.to_string_lossy().trim_end_matches('/'));
    // Seatbelt matches exec against the resolved path; Homebrew's limactl is a symlink into Cellar.
    let real = limactl
        .canonicalize()
        .unwrap_or_else(|_| limactl.to_path_buf());
    let exe = if real == limactl {
        format!("(literal {})", sb_string(&limactl.to_string_lossy()))
    } else {
        format!(
            "(literal {}) (literal {})",
            sb_string(&limactl.to_string_lossy()),
            sb_string(&real.to_string_lossy())
        )
    };
    format!(
        "(version 1)\n(allow default)\n(deny network-outbound)\n\
(allow network-outbound (remote unix-socket (subpath {dir})))\n\
(deny mach-lookup (global-name \"com.apple.dnssd.service\"))\n\
(allow network-outbound (remote ip \"localhost:{ssh_port}\"))\n\
(deny network-bind)\n\
(allow network-bind (local tcp \"localhost:{ssh_port}\"))\n\
(allow network-bind (local unix-socket (subpath {dir})))\n\
(deny file-write*)\n\
(allow file-write* (subpath {dir}) (subpath {config}) (subpath {tmp}) (literal \"/dev/null\") (regex #\"^/dev/tty\") (literal \"/dev/ptmx\"))\n\
(deny process-exec)\n\
(allow process-exec {exe} (literal \"/usr/bin/ssh\") (literal \"/usr/bin/sw_vers\") (literal \"/usr/sbin/system_profiler\") (literal \"/bin/sh\") (literal \"/bin/bash\"))\n"
    )
}

/// `confstr(_CS_DARWIN_USER_TEMP_DIR)`: the per-user `/var/folders/.../T/`, resolved.
pub fn user_temp_dir() -> std::path::PathBuf {
    let mut buf = vec![0u8; 1024];
    // SAFETY: confstr writes at most buf.len() bytes, NUL-terminated, and returns the needed size.
    let n = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    let raw = if n > 0 && n <= buf.len() {
        String::from_utf8_lossy(&buf[..n - 1]).into_owned()
    } else {
        std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())
    };
    let p = std::path::PathBuf::from(raw);
    p.canonicalize().unwrap_or(p)
}

/// The `-R` proxy forward: outbound to the guest's SSH port and the host proxy port only.
pub fn fwd_proxy_profile(ssh_port: u16, proxy_port: u16) -> String {
    format!(
        "(version 1)\n(allow default)\n(deny network-outbound)\n\
(allow network-outbound (remote ip \"localhost:{ssh_port}\"))\n\
(allow network-outbound (remote ip \"localhost:{proxy_port}\"))\n\
(deny network-bind)\n\
(deny mach-lookup (global-name \"com.apple.dnssd.service\"))\n"
    )
}

/// The `-L` web forward: outbound to the SSH port only; binds only the web port.
pub fn fwd_web_profile(ssh_port: u16, web_port: u16) -> String {
    format!(
        "(version 1)\n(allow default)\n(deny network-outbound)\n\
(allow network-outbound (remote ip \"localhost:{ssh_port}\"))\n\
(deny network-bind)\n\
(allow network-bind (local tcp \"localhost:{web_port}\"))\n\
(deny mach-lookup (global-name \"com.apple.dnssd.service\"))\n"
    )
}

/// A quoted SBPL string literal. Paths come from validated config, but quote anyway.
fn sb_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostagent_profile_matches_fixture() {
        let want = include_str!("../../tests/fixtures/hostagent.sb");
        assert_eq!(
            hostagent_profile(
                Path::new("/x/.lima/bunker-t"),
                60100,
                Path::new("/x/bin/limactl"),
                Path::new("/var/folders/ab/T/")
            ),
            want
        );
    }

    fn rules(p: &str, prefix: &str) -> Vec<String> {
        p.lines()
            .filter(|l| l.starts_with(prefix))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn fwd_profiles_allow_only_their_ports() {
        let p = fwd_proxy_profile(60100, 18800);
        assert_eq!(
            rules(&p, "(allow network-outbound"),
            [
                "(allow network-outbound (remote ip \"localhost:60100\"))",
                "(allow network-outbound (remote ip \"localhost:18800\"))"
            ]
        );
        assert!(rules(&p, "(allow network-bind").is_empty(), "{p}");
        assert!(p.contains("(deny network-outbound)") && p.contains("(deny network-bind)"));
        let w = fwd_web_profile(60100, 18900);
        assert_eq!(
            rules(&w, "(allow network-outbound"),
            ["(allow network-outbound (remote ip \"localhost:60100\"))"]
        );
        assert_eq!(
            rules(&w, "(allow network-bind"),
            ["(allow network-bind (local tcp \"localhost:18900\"))"]
        );
        for prof in [&p, &w] {
            assert!(prof.contains("(deny mach-lookup (global-name \"com.apple.dnssd.service\"))"));
        }
    }

    fn nc(profile: Option<&str>, args: &[&str]) -> bool {
        let mut c = match profile {
            Some(p) => {
                let mut c = std::process::Command::new("/usr/bin/sandbox-exec");
                c.args(["-p", p, "/usr/bin/nc"]);
                c
            }
            None => std::process::Command::new("/usr/bin/nc"),
        };
        c.args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        c.status().unwrap().success()
    }

    #[test]
    #[ignore]
    fn sys_fwd_profile_confines_nc() {
        let ssh = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ssh_port = ssh.local_addr().unwrap().port();
        let web = fwd_web_profile(ssh_port, 18999);
        let proxy = fwd_proxy_profile(ssh_port, 18899);
        assert!(
            nc(None, &["-z", "-w2", "1.1.1.1", "443"]),
            "control: no internet unconfined, test proves nothing"
        );
        for p in [&web, &proxy] {
            assert!(
                !nc(Some(p), &["-z", "-w2", "1.1.1.1", "443"]),
                "internet reachable under\n{p}"
            );
            assert!(
                nc(Some(p), &["-z", "-w2", "127.0.0.1", &ssh_port.to_string()]),
                "ssh port blocked under\n{p}"
            );
        }
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let other_port = other.local_addr().unwrap().port().to_string();
        assert!(
            nc(None, &["-z", "-w2", "127.0.0.1", &other_port]),
            "control: local listener unreachable"
        );
        assert!(
            !nc(Some(&web), &["-z", "-w2", "127.0.0.1", &other_port]),
            "other loopback port reachable"
        );
    }

    fn run_under(profile: &str, args: &[&str]) -> bool {
        std::process::Command::new("/usr/bin/sandbox-exec")
            .args(["-p", profile])
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    #[test]
    #[ignore]
    fn sys_hardened_profile_blocks_home_writes() {
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
        let lima = tempfile::tempdir_in(&home).unwrap();
        let vm_dir = lima.path().canonicalize().unwrap().join("bunker-t");
        std::fs::create_dir_all(&vm_dir).unwrap();
        let tmp = user_temp_dir();
        let probe = home.join("bunker-sbx-probe");
        let _ = std::fs::remove_file(&probe);
        // "limactl" = touch, so only the file rules decide.
        let p = hostagent_profile(&vm_dir, 60100, Path::new("/usr/bin/touch"), &tmp);
        assert!(
            run_under(
                "(version 1)(allow default)",
                &["/usr/bin/touch", probe.to_str().unwrap()]
            ),
            "control: home not writable"
        );
        std::fs::remove_file(&probe).unwrap();
        assert!(
            !run_under(&p, &["/usr/bin/touch", probe.to_str().unwrap()]),
            "home write allowed"
        );
        assert!(!probe.exists());
        assert!(
            run_under(
                &p,
                &["/usr/bin/touch", vm_dir.join("sbx-probe").to_str().unwrap()]
            ),
            "vm dir write denied"
        );
        assert!(
            run_under(
                &p,
                &[
                    "/usr/bin/touch",
                    tmp.join("bunker-sbx-probe").to_str().unwrap()
                ]
            ),
            "temp dir write denied"
        );
        // Exec: only limactl and ssh.
        let p = hostagent_profile(&vm_dir, 60100, Path::new("/usr/bin/true"), &tmp);
        assert!(run_under(&p, &["/usr/bin/true"]));
        assert!(
            !run_under(&p, &["/usr/bin/touch", vm_dir.join("x").to_str().unwrap()]),
            "exec of another binary allowed"
        );
        // Networking still denied ("limactl" = curl).
        let p = hostagent_profile(&vm_dir, 60100, Path::new("/usr/bin/curl"), &tmp);
        assert!(
            run_under(
                "(version 1)(allow default)",
                &["/usr/bin/curl", "-sI", "-m", "5", "https://example.com"]
            ),
            "control: no internet"
        );
        assert!(
            !run_under(
                &p,
                &["/usr/bin/curl", "-sI", "-m", "5", "https://example.com"]
            ),
            "network allowed"
        );
        let _ = std::fs::remove_file(tmp.join("bunker-sbx-probe"));
    }
}
