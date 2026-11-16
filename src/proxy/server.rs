//! The proxy: one listener per project, CONNECT and absolute-URI HTTP, policy per request.
//! Order per request: parse the target, load the policy (fail closed), refuse internal literals,
//! decide, resolve, refuse if any resolved address is internal, then connect only to those.
use crate::config::Config;
use crate::config::validate::is_dns_name;
use crate::error::{BunkerError, Result};
use crate::net::log::{EventKind, NetEvent, append_event};
use crate::paths::ProjectDir;
use crate::proxy::addr::is_internal;
use crate::proxy::lease::{LeaseKind, lease_active};
use crate::proxy::policy::{Phases, Policy, Verdict};
use crate::proxy::resolve::{Connector, Resolver};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::header::{HOST, HeaderName, HeaderValue};
use hyper::{HeaderMap, Method, Request, Response, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::net::{TcpListener, TcpStream};

pub type OwnAddrs = Arc<dyn Fn() -> Vec<IpAddr> + Send + Sync>;
type Body = BoxBody<Bytes, hyper::Error>;

/// One project's listener, already bound so callers (and tests) know its address.
pub struct ProjectListener {
    pub project: String,
    pub listener: std::net::TcpListener,
    pub dir: ProjectDir,
}

pub async fn serve(
    listeners: Vec<ProjectListener>,
    resolver: Arc<dyn Resolver>,
    connector: Arc<dyn Connector>,
    own: OwnAddrs,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<()> {
    let mut tasks = tokio::task::JoinSet::new();
    for pl in listeners {
        let io = |e| BunkerError::io(format!("cannot listen for {}", pl.project), e);
        pl.listener.set_nonblocking(true).map_err(io)?;
        let listener = TcpListener::from_std(pl.listener).map_err(io)?;
        let ctx = Arc::new(Ctx {
            project: pl.project,
            dir: pl.dir,
            state: Mutex::new(State::default()),
            resolver: resolver.clone(),
            connector: connector.clone(),
            own: own.clone(),
        });
        tasks.spawn(accept_loop(listener, ctx));
    }
    shutdown.await;
    tasks.abort_all();
    Ok(())
}

async fn accept_loop(listener: TcpListener, ctx: Arc<Ctx>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(move |req| {
                let ctx = ctx.clone();
                async move { Ok::<_, Infallible>(ctx.handle(req).await) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), svc)
                .with_upgrades()
                .await;
        });
    }
}

#[derive(Default)]
struct State {
    /// (mtime, length) of bunker.toml at the last load; `None` before the first.
    key: Option<Option<(SystemTime, u64)>>,
    /// `None` means the file is missing or invalid: block everything.
    policy: Option<Policy>,
}

struct Ctx {
    project: String,
    dir: ProjectDir,
    state: Mutex<State>,
    resolver: Arc<dyn Resolver>,
    connector: Arc<dyn Connector>,
    own: OwnAddrs,
}

enum Blocked {
    Reason(&'static str),
    Internal,
    PolicyError,
}

impl Ctx {
    fn log(&self, kind: EventKind) {
        let _ = append_event(&self.dir.network_log(), &NetEvent::now(&self.project, kind));
    }

    /// Reloads when bunker.toml's mtime or length changed; logs proxy_load or policy_error once per change.
    fn policy(&self) -> Option<Policy> {
        let path = self.dir.config();
        let key = std::fs::metadata(&path)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        let mut st = self.state.lock().unwrap();
        if st.key == Some(key) {
            return st.policy.clone();
        }
        st.key = Some(key);
        match Config::load(&self.dir) {
            Ok(cfg) => {
                st.policy = Some(Policy::from_network(&cfg.network));
                self.log(EventKind::Policy {
                    source: "proxy_load".into(),
                    allow: cfg.network.allow,
                    build: cfg.network.build,
                    deny: cfg.network.deny,
                });
            }
            Err(e) => {
                st.policy = None;
                self.log(EventKind::PolicyError { message: e.message });
            }
        }
        st.policy.clone()
    }

    fn phases(&self) -> Phases {
        let now = SystemTime::now();
        Phases {
            build: lease_active(&self.dir, LeaseKind::Build, now),
            maintenance: lease_active(&self.dir, LeaseKind::Maintenance, now),
        }
    }

    fn decision(&self, host: &str, port: u16, phases: Phases, verdict: &str, reason: &str) {
        let phase = match (phases.build, phases.maintenance) {
            (true, true) => "build+maintenance",
            (true, false) => "build",
            (false, true) => "maintenance",
            (false, false) => "run",
        };
        self.log(EventKind::Decision {
            host: host.into(),
            port,
            phase: phase.into(),
            verdict: verdict.into(),
            reason: reason.into(),
        });
    }

    fn blocked(&self, host: &str, port: u16, phases: Phases, why: Blocked) -> Response<Body> {
        let (reason, body) = match why {
            Blocked::Reason(r) => (
                r,
                format!(
                    "bunker: blocked {host}:{port} ({r}). To allow it: bunker net allow {host} {}\n",
                    self.project
                ),
            ),
            Blocked::Internal => (
                "internal_address",
                format!("bunker: blocked {host}:{port}: internal addresses are never allowed\n"),
            ),
            Blocked::PolicyError => (
                "policy_error",
                format!(
                    "bunker: blocked {host}:{port} (policy_error): ~/.bunker/{p}/bunker.toml is invalid. Fix it, then run bunker net reload {p}\n",
                    p = self.project
                ),
            ),
        };
        self.decision(host, port, phases, "block", reason);
        text(StatusCode::FORBIDDEN, body)
    }

    async fn handle(&self, mut req: Request<Incoming>) -> Response<Body> {
        let Some((host, port)) = target(&req) else {
            return text(
                StatusCode::BAD_REQUEST,
                "bunker: send CONNECT host:port or an absolute http:// URI to a valid host\n"
                    .into(),
            );
        };
        let phases = self.phases();
        let Some(policy) = self.policy() else {
            return self.blocked(&host, port, phases, Blocked::PolicyError);
        };
        let own = (self.own)();
        let literal: Option<IpAddr> = host.parse().ok();
        if literal.is_some_and(|ip| is_internal(ip, &own)) {
            return self.blocked(&host, port, phases, Blocked::Internal);
        }
        let d = policy.decide(&host, port, phases);
        if d.verdict == Verdict::Block {
            return self.blocked(&host, port, phases, Blocked::Reason(d.reason.as_str()));
        }
        let addrs: Vec<SocketAddr> = match literal {
            Some(ip) => vec![SocketAddr::new(ip, port)],
            None => {
                let r = tokio::time::timeout(
                    Duration::from_secs(10),
                    self.resolver.resolve(&host, port),
                )
                .await;
                match r {
                    Ok(Ok(a)) if !a.is_empty() => a,
                    _ => {
                        self.decision(&host, port, phases, "block", "resolve_failed");
                        return text(
                            StatusCode::BAD_GATEWAY,
                            format!("bunker: cannot resolve {host}\n"),
                        );
                    }
                }
            }
        };
        if addrs.iter().any(|a| is_internal(a.ip(), &own)) {
            return self.blocked(&host, port, phases, Blocked::Internal);
        }
        self.decision(&host, port, phases, "allow", d.reason.as_str());
        let mut upstream = None;
        for a in connect_order(&addrs) {
            // A route that swallows packets (often IPv6 at home) costs one short attempt.
            if let Ok(Ok(s)) = tokio::time::timeout(ATTEMPT, self.connector.connect(a)).await {
                upstream = Some(s);
                break;
            }
        }
        let Some(upstream) = upstream else {
            return text(
                StatusCode::BAD_GATEWAY,
                format!("bunker: cannot connect to {host}:{port}\n"),
            );
        };
        if req.method() == Method::CONNECT {
            let on_upgrade = hyper::upgrade::on(&mut req);
            tokio::spawn(async move {
                if let Ok(up) = on_upgrade.await {
                    let mut up = TokioIo::new(up);
                    let mut upstream = upstream;
                    let _ = tokio::io::copy_bidirectional(&mut up, &mut upstream).await;
                }
            });
            return Response::new(empty());
        }
        forward(req, upstream, &host, port).await
    }
}

/// `(host, port)` from a CONNECT authority or an absolute `http://` URI, with the host normalized
/// and checked to be a hostname or IP literal. Anything else (origin-form, garbage) is `None`.
fn target(req: &Request<Incoming>) -> Option<(String, u16)> {
    let uri = req.uri();
    let auth = uri.authority()?;
    let port = if req.method() == Method::CONNECT {
        auth.port_u16()?
    } else {
        if uri.scheme_str() != Some("http") {
            return None;
        }
        auth.port_u16().unwrap_or(80)
    };
    if port == 0 {
        return None;
    }
    let raw = auth.host();
    let raw = raw
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(raw);
    let host = crate::net::edit::normalize_host(raw);
    let ok = host.parse::<IpAddr>().is_ok() || is_dns_name(&host);
    ok.then_some((host, port))
}

const HOP_BY_HOP: [&str; 7] = [
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "proxy-connection",
];

fn strip_hop_by_hop(h: &mut HeaderMap) {
    let named: Vec<HeaderName> = h
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|n| HeaderName::from_bytes(n.trim().as_bytes()).ok())
        .collect();
    for n in named {
        h.remove(n);
    }
    for n in HOP_BY_HOP {
        h.remove(n);
    }
    let proxy: Vec<HeaderName> = h
        .keys()
        .filter(|k| k.as_str().starts_with("proxy-"))
        .cloned()
        .collect();
    for n in proxy {
        h.remove(n);
    }
}

/// How long one address gets before the next is tried.
const ATTEMPT: std::time::Duration = std::time::Duration::from_secs(3);

/// The checked addresses, IPv4 first and then alternating families, so a broken IPv6 route
/// doesn't stall a host that resolves IPv6 first.
fn connect_order(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    let (v4, v6): (Vec<SocketAddr>, Vec<SocketAddr>) = addrs.iter().partition(|a| a.is_ipv4());
    let mut out = Vec::with_capacity(addrs.len());
    for i in 0..v4.len().max(v6.len()) {
        out.extend(v4.get(i));
        out.extend(v6.get(i));
    }
    out
}

async fn forward(
    req: Request<Incoming>,
    upstream: TcpStream,
    host: &str,
    port: u16,
) -> Response<Body> {
    let bad_gateway = || {
        text(
            StatusCode::BAD_GATEWAY,
            format!("bunker: upstream {host}:{port} failed\n"),
        )
    };
    let Ok((mut sender, conn)) =
        hyper::client::conn::http1::handshake(TokioIo::new(upstream)).await
    else {
        return bad_gateway();
    };
    tokio::spawn(conn);
    let (mut parts, body) = req.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    parts.uri = match path.parse::<Uri>() {
        Ok(u) => u,
        Err(_) => return bad_gateway(),
    };
    strip_hop_by_hop(&mut parts.headers);
    // Always the checked target: a client-sent Host could name another site behind the same IP
    // (a CDN), which would bypass the allowlist.
    let value = if port == 80 {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };
    match HeaderValue::from_str(&value) {
        Ok(v) => {
            parts.headers.insert(HOST, v);
        }
        Err(_) => return bad_gateway(),
    }
    match sender.send_request(Request::from_parts(parts, body)).await {
        Ok(resp) => {
            let (mut parts, body) = resp.into_parts();
            strip_hop_by_hop(&mut parts.headers);
            Response::from_parts(parts, body.boxed())
        }
        Err(_) => bad_gateway(),
    }
}

fn text(status: StatusCode, body: String) -> Response<Body> {
    let mut r = Response::new(
        Full::new(Bytes::from(body))
            .map_err(|n: Infallible| match n {})
            .boxed(),
    );
    *r.status_mut() = status;
    r.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

fn empty() -> Body {
    Empty::<Bytes>::new()
        .map_err(|n: Infallible| match n {})
        .boxed()
}
