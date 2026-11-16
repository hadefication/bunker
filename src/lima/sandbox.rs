//! Seatbelt profiles. They restrict networking only (see the spec's threat model); each starts
//! from `(allow default)`. Ports are `localhost:<port>`, which covers 127.0.0.1 and ::1.
use std::path::Path;

/// The spike's final hostagent profile: Lima's unix sockets in the VM dir, and the SSH port.
pub fn hostagent_profile(lima_vm_dir_real: &Path, ssh_port: u16) -> String {
    let dir = sb_string(&lima_vm_dir_real.to_string_lossy());
    format!(
        "(version 1)\n(allow default)\n(deny network-outbound)\n\
(allow network-outbound (remote unix-socket (subpath {dir})))\n\
(deny mach-lookup (global-name \"com.apple.dnssd.service\"))\n\
(allow network-outbound (remote ip \"localhost:{ssh_port}\"))\n\
(deny network-bind)\n\
(allow network-bind (local tcp \"localhost:{ssh_port}\"))\n\
(allow network-bind (local unix-socket (subpath {dir})))\n"
    )
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
    fn hostagent_profile_matches_spike_final() {
        let want = include_str!("../../tests/fixtures/hostagent.sb");
        assert_eq!(
            hostagent_profile(Path::new("/x/.lima/bunker-t"), 60100),
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
}
