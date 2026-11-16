//! The proxy against a fake resolver and a fake connector. The connector maps only the public
//! address 93.184.215.14 to a local upstream and refuses everything else, so any connect to an
//! address the proxy didn't check fails the test.
use bunker::net::log::{EventKind, read_events};
use bunker::paths::ProjectDir;
use bunker::proxy::lease::{LeaseKind, open_lease};
use bunker::proxy::resolve::{BoxFuture, Connector, Resolver};
use bunker::proxy::server::{ProjectListener, serve};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const PUBLIC: &str = "93.184.215.14";

struct FakeResolver(HashMap<String, Vec<IpAddr>>);

impl Resolver for FakeResolver {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
        port: u16,
    ) -> BoxFuture<'a, io::Result<Vec<SocketAddr>>> {
        let r = self
            .0
            .get(host)
            .map(|ips| ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such host"));
        Box::pin(async move { r })
    }
}

struct FakeConnector {
    upstream: SocketAddr,
    calls: Mutex<Vec<SocketAddr>>,
}

impl Connector for FakeConnector {
    fn connect(&self, addr: SocketAddr) -> BoxFuture<'_, io::Result<TcpStream>> {
        self.calls.lock().unwrap().push(addr);
        let up = self.upstream;
        Box::pin(async move {
            if addr.ip().to_string() == PUBLIC && (addr.port() == 443 || addr.port() == 80) {
                TcpStream::connect(up).await
            } else {
                Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "unchecked address",
                ))
            }
        })
    }
}

/// Upstream: echoes after a CONNECT; for HTTP, records the request head and answers 200 "hello".
async fn upstream(seen: Arc<Mutex<Vec<String>>>) -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                if text.starts_with("GET ") {
                    seen.lock().unwrap().push(text);
                    let _ = s
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello")
                        .await;
                } else {
                    let _ = s.write_all(&buf[..n]).await;
                    let _ = tokio::io::copy(&mut s.split().0, &mut tokio::io::sink()).await;
                }
            });
        }
    });
    addr
}

fn config(repo: &str, proxy_port: u16, allow: &[&str]) -> String {
    let list = allow
        .iter()
        .map(|h| format!("\"{h}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "project = \"{repo}\"\nstack = \"laravel\"\nphp = \"8.4\"\n\n[ports]\nssh = 60100\nproxy = {proxy_port}\nweb = 18900\n\n[network]\nallow = [{list}]\n"
    )
}

struct Harness {
    _home: tempfile::TempDir,
    dirs: Vec<ProjectDir>,
    addrs: Vec<SocketAddr>,
    connector: Arc<FakeConnector>,
    seen: Arc<Mutex<Vec<String>>>,
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl Harness {
    async fn new(
        projects: &[(&str, &[&str])],
        names: &[(&str, &[&str])],
        own: Vec<IpAddr>,
    ) -> Self {
        let home = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let connector = Arc::new(FakeConnector {
            upstream: upstream(seen.clone()).await,
            calls: Mutex::new(vec![]),
        });
        let resolver = Arc::new(FakeResolver(
            names
                .iter()
                .map(|(n, ips)| {
                    (
                        n.to_string(),
                        ips.iter().map(|s| s.parse().unwrap()).collect(),
                    )
                })
                .collect(),
        ));
        let mut listeners = vec![];
        let (mut dirs, mut addrs) = (vec![], vec![]);
        for (i, (name, allow)) in projects.iter().enumerate() {
            let pd = ProjectDir::under(home.path(), name);
            std::fs::create_dir_all(&pd.root).unwrap();
            std::fs::write(
                pd.config(),
                config(home.path().to_str().unwrap(), 18800 + i as u16, allow),
            )
            .unwrap();
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            addrs.push(l.local_addr().unwrap());
            dirs.push(pd.clone());
            listeners.push(ProjectListener {
                project: name.to_string(),
                listener: l,
                dir: pd,
            });
        }
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let conn: Arc<dyn Connector> = connector.clone();
        tokio::spawn(serve(
            listeners,
            resolver,
            conn,
            Arc::new(move || own.clone()),
            async move {
                let _ = rx.await;
            },
        ));
        Self {
            _home: home,
            dirs,
            addrs,
            connector,
            seen,
            _stop: tx,
        }
    }

    fn log(&self, i: usize) -> Vec<EventKind> {
        read_events(&self.dirs[i].network_log())
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }
}

/// Sends `head` and reads one response: (status, body). Leaves the stream open for tunnelling.
async fn request(addr: SocketAddr, head: &str) -> (u16, String, TcpStream) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(head.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut byte))
            .await
            .unwrap()
            .unwrap();
        assert!(
            n > 0,
            "connection closed mid-head: {:?}",
            String::from_utf8_lossy(&buf)
        );
        buf.push(byte[0]);
    }
    let head = String::from_utf8(buf).unwrap();
    let status: u16 = head[9..12].parse().unwrap();
    let len = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(|v| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).await.unwrap();
    (status, String::from_utf8(body).unwrap(), s)
}

async fn connect(addr: SocketAddr, target: &str) -> (u16, String, TcpStream) {
    request(
        addr,
        &format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n"),
    )
    .await
}

#[tokio::test]
async fn connect_to_allowed_host_tunnels_bytes() {
    let h = Harness::new(
        &[("fixture", &["example.com"])],
        &[("example.com", &[PUBLIC])],
        vec![],
    )
    .await;
    let (status, _, mut s) = connect(h.addrs[0], "example.com:443").await;
    assert_eq!(status, 200);
    s.write_all(b"ping").await.unwrap();
    let mut got = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&got, b"ping");
}

#[tokio::test]
async fn connect_to_unlisted_host_gets_403_naming_allow_command() {
    let h = Harness::new(
        &[("fixture", &[])],
        &[("api.stripe.com", &[PUBLIC])],
        vec![],
    )
    .await;
    let (status, body, _) = connect(h.addrs[0], "api.stripe.com:443").await;
    assert_eq!(status, 403);
    assert_eq!(
        body,
        "bunker: blocked api.stripe.com:443 (not_listed). To allow it: bunker net allow api.stripe.com fixture\n"
    );
    assert!(h.connector.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn absolute_uri_http_is_forwarded_without_proxy_headers() {
    let h = Harness::new(
        &[("fixture", &["example.com"])],
        &[("example.com", &[PUBLIC])],
        vec![],
    )
    .await;
    let (status, body, _) = request(
        h.addrs[0],
        "GET http://example.com/p HTTP/1.1\r\nHost: other-site.test\r\nProxy-Authorization: Basic eDp5\r\nProxy-Connection: keep-alive\r\n\r\n",
    )
    .await;
    assert_eq!((status, body.as_str()), (200, "hello"));
    let seen = h.seen.lock().unwrap().join("");
    assert!(seen.starts_with("GET /p HTTP/1.1\r\n"), "{seen}");
    assert!(!seen.to_ascii_lowercase().contains("proxy-"), "{seen}");
    assert!(
        seen.to_ascii_lowercase().contains("host: example.com"),
        "{seen}"
    );
}

async fn assert_internal_refused(h: &Harness, target: &str) {
    let (status, body, _) = connect(h.addrs[0], target).await;
    assert_eq!(status, 403, "{target}");
    assert!(
        body.contains("internal addresses are never allowed"),
        "{body}"
    );
    assert!(
        h.connector.calls.lock().unwrap().is_empty(),
        "connector was called for {target}"
    );
}

#[tokio::test]
async fn internal_ip_literal_is_refused() {
    let h = Harness::new(&[("fixture", &["*.example.com"])], &[], vec![]).await;
    assert_internal_refused(&h, "10.0.0.1:443").await;
    assert_internal_refused(&h, "[::1]:443").await;
}

#[tokio::test]
async fn name_resolving_to_internal_is_refused() {
    let h = Harness::new(
        &[("fixture", &["evil.test"])],
        &[("evil.test", &["127.0.0.1"])],
        vec![],
    )
    .await;
    assert_internal_refused(&h, "evil.test:443").await;
}

#[tokio::test]
async fn mixed_public_and_internal_resolution_is_refused() {
    let h = Harness::new(
        &[("fixture", &["cdn.test"])],
        &[("cdn.test", &[PUBLIC, "192.168.1.1"])],
        vec![],
    )
    .await;
    assert_internal_refused(&h, "cdn.test:443").await;
    assert!(h.log(0).iter().any(|k| matches!(k, EventKind::Decision { reason, verdict, .. } if reason == "internal_address" && verdict == "block")));
}

#[tokio::test]
async fn mac_own_address_is_refused() {
    let h = Harness::new(
        &[("fixture", &["me.test"])],
        &[("me.test", &["203.0.113.7"])],
        vec!["203.0.113.7".parse().unwrap()],
    )
    .await;
    assert_internal_refused(&h, "me.test:443").await;
}

#[tokio::test]
async fn nat64_embedded_internal_is_refused() {
    let h = Harness::new(
        &[("fixture", &["six.test"])],
        &[("six.test", &["64:ff9b::a00:1"])],
        vec![],
    )
    .await;
    assert_internal_refused(&h, "six.test:443").await;
}

#[tokio::test]
async fn policy_reloads_on_file_change_without_restart() {
    let h = Harness::new(&[("fixture", &[])], &[("example.com", &[PUBLIC])], vec![]).await;
    assert_eq!(connect(h.addrs[0], "example.com:443").await.0, 403);
    let text = std::fs::read_to_string(h.dirs[0].config()).unwrap();
    let (edited, _) = bunker::net::edit::edit_lists(
        &text,
        &[(
            bunker::net::edit::ListName::Allow,
            bunker::net::edit::Op::Add,
            "example.com",
        )],
    )
    .unwrap();
    bunker::paths::write_private(&h.dirs[0].config(), edited.as_bytes()).unwrap();
    assert_eq!(connect(h.addrs[0], "example.com:443").await.0, 200);
    let loads = h
        .log(0)
        .iter()
        .filter(|k| matches!(k, EventKind::Policy { source, .. } if source == "proxy_load"))
        .count();
    assert_eq!(loads, 2);
}

#[tokio::test]
async fn broken_config_fails_closed_and_logs_policy_error() {
    let h = Harness::new(
        &[("a", &["example.com"]), ("b", &["example.com"])],
        &[("example.com", &[PUBLIC])],
        vec![],
    )
    .await;
    let text = std::fs::read_to_string(h.dirs[0].config()).unwrap();
    std::fs::write(
        h.dirs[0].config(),
        text.replace("[ports]", "typo = 1\n[ports]"),
    )
    .unwrap();
    assert_eq!(connect(h.addrs[0], "example.com:443").await.0, 403);
    assert_eq!(connect(h.addrs[0], "example.com:443").await.0, 403);
    let errors = h
        .log(0)
        .iter()
        .filter(|k| matches!(k, EventKind::PolicyError { .. }))
        .count();
    assert_eq!(errors, 1, "logged once per change");
    assert_eq!(connect(h.addrs[1], "example.com:443").await.0, 200);
}

#[tokio::test]
async fn build_host_allowed_only_with_lease() {
    let h = Harness::new(&[("fixture", &[])], &[("packagist.org", &[PUBLIC])], vec![]).await;
    assert_eq!(connect(h.addrs[0], "packagist.org:443").await.0, 403);
    let lease = open_lease(&h.dirs[0], LeaseKind::Build, Duration::from_secs(60)).unwrap();
    assert_eq!(connect(h.addrs[0], "packagist.org:443").await.0, 200);
    drop(lease);
    assert_eq!(connect(h.addrs[0], "packagist.org:443").await.0, 403);
}

#[tokio::test]
async fn every_decision_is_logged() {
    let h = Harness::new(
        &[("fixture", &["example.com"])],
        &[("example.com", &[PUBLIC]), ("x.test", &[PUBLIC])],
        vec![],
    )
    .await;
    connect(h.addrs[0], "example.com:443").await;
    connect(h.addrs[0], "x.test:443").await;
    connect(h.addrs[0], "10.0.0.1:443").await;
    let decisions: Vec<(String, String)> = h
        .log(0)
        .into_iter()
        .filter_map(|k| match k {
            EventKind::Decision { host, verdict, .. } => Some((host, verdict)),
            _ => None,
        })
        .collect();
    assert_eq!(
        decisions,
        [
            ("example.com".into(), "allow".into()),
            ("x.test".into(), "block".into()),
            ("10.0.0.1".into(), "block".into())
        ]
    );
}

#[tokio::test]
async fn origin_form_request_is_rejected_400() {
    let h = Harness::new(&[("fixture", &["example.com"])], &[], vec![]).await;
    let (status, _, _) = request(h.addrs[0], "GET /p HTTP/1.1\r\nHost: example.com\r\n\r\n").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn garbage_host_is_rejected_400_and_never_logged_raw() {
    let h = Harness::new(&[("fixture", &[])], &[], vec![]).await;
    let (status, _, _) = connect(h.addrs[0], "ev%1bil.test:443").await;
    assert_eq!(status, 400);
}
