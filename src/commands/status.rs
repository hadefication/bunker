//! `bunker status [project]`: the VM and its confinement, the forwards, the proxy, the tunnel,
//! the guest's services and the live release, for one project or all of them.
use crate::config::Config;
use crate::deploy::steps::{BASE, is_release_name, releases_script};
use crate::error::Result;
use crate::host::Host;
use crate::launchd::{AgentState, Role, label};
use crate::lima::confine::Confinement;
use crate::lima::vm::{instance_status, shell, vm_name};
use crate::net::log::{EventKind, read_events};
use crate::output::{ExitCode, Report};
use crate::paths::ProjectDir;
use serde_json::{Value, json};
use std::io::{self, Write};
use std::time::Duration;

pub struct StatusReport {
    pub items: Vec<Value>,
    /// One project asked for: the report is that object, not an array.
    pub single: bool,
    pub exit: ExitCode,
}

impl Report for StatusReport {
    fn human(&self, w: &mut dyn Write) -> io::Result<()> {
        if self.items.is_empty() {
            return writeln!(
                w,
                "no bunker projects (bunker init <project> --path <repo>)"
            );
        }
        for v in &self.items {
            human_one(v, w)?;
        }
        Ok(())
    }
    fn json(&self) -> Value {
        if self.single {
            self.items.first().cloned().unwrap_or(Value::Null)
        } else {
            Value::Array(self.items.clone())
        }
    }
    fn exit(&self) -> ExitCode {
        self.exit
    }
}

fn human_one(v: &Value, w: &mut dyn Write) -> io::Result<()> {
    let s = |p: &str| {
        v.pointer(p)
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default()
    };
    writeln!(
        w,
        "{}: {}, {}",
        s("/project"),
        s("/vm/state"),
        s("/vm/confinement")
    )?;
    let fwd = |k: &str| {
        if v.pointer(&format!("/forwards/{k}/loaded")) == Some(&Value::Bool(true)) {
            "up"
        } else {
            "down"
        }
    };
    writeln!(w, "  forwards  proxy {}, web {}", fwd("proxy"), fwd("web"))?;
    writeln!(
        w,
        "  proxy     {}, policy {}",
        if v["proxy"]["listening"] == true {
            "listening"
        } else {
            "not listening"
        },
        s("/proxy/policy")
    )?;
    if v["tunnel"].is_object() {
        writeln!(
            w,
            "  tunnel    {}",
            if v["tunnel"]["loaded"] == true {
                "running"
            } else {
                "not running"
            }
        )?;
    }
    let mut svc = vec![
        format!("nginx {}", s("/services/nginx")),
        format!("php-fpm {}", s("/services/php_fpm")),
        format!("mysql {}", s("/services/mysql")),
    ];
    if let Some(ws) = v["services"]["workers"].as_object() {
        for (k, st) in ws {
            svc.push(format!("{k} {}", st.as_str().unwrap_or("?")));
        }
    }
    writeln!(w, "  services  {}", svc.join(", "))?;
    match v["release"].as_object() {
        Some(r) => writeln!(
            w,
            "  release   {} (previous: {})",
            r["current"].as_str().unwrap_or("?"),
            r["previous"]
                .as_str()
                .filter(|p| !p.is_empty())
                .unwrap_or("none")
        )?,
        None => writeln!(w, "  release   none deployed")?,
    }
    for x in v["warnings"].as_array().into_iter().flatten() {
        writeln!(w, "  warning: {}", x.as_str().unwrap_or(""))?;
    }
    Ok(())
}

/// Project directories under the bunker home with a bunker.toml, sorted.
fn all_projects(h: &Host) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(&h.home)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            crate::config::validate::validate_project_name(&name).ok()?;
            e.path().join("bunker.toml").is_file().then_some(name)
        })
        .collect();
    v.sort();
    v
}

pub fn status(h: &Host, project: Option<&str>) -> Result<StatusReport> {
    let names = match project {
        Some(p) => {
            Config::load(&ProjectDir::under(&h.home, p))?;
            vec![p.to_string()]
        }
        None => all_projects(h),
    };
    let (mut unconfined, mut down) = (false, false);
    let mut items = Vec::new();
    for name in &names {
        let one = match one(h, name) {
            Ok(o) => o,
            Err(e) if project.is_none() => One {
                value: json!({"project": name, "error": e.message, "warnings": [e.message]}),
                unconfined: false,
                down: true,
            },
            Err(e) => return Err(e),
        };
        unconfined |= one.unconfined;
        down |= one.down;
        items.push(one.value);
    }
    Ok(StatusReport {
        items,
        single: project.is_some(),
        exit: if unconfined {
            ExitCode::Unconfined
        } else if down {
            ExitCode::Failure
        } else {
            ExitCode::Ok
        },
    })
}

struct One {
    value: Value,
    unconfined: bool,
    down: bool,
}

fn agent(st: &AgentState) -> Value {
    json!({"loaded": st.loaded, "pid": st.pid})
}

fn one(h: &Host, name: &str) -> Result<One> {
    let pd = ProjectDir::under(&h.home, name);
    let cfg = Config::load(&pd)?;
    let vm = vm_name(name);
    let mut warnings: Vec<String> = Vec::new();
    let mut down = false;

    let state = match instance_status(h, &vm)? {
        Some(s) => s.to_ascii_lowercase(),
        None => "absent".to_string(),
    };
    let running = state == "running";
    if !running {
        warnings.push(format!("the VM is {state}"));
        down = true;
    }
    let conf = h.probe.confinement(name)?;
    let (unconfined, hostagent) = match &conf {
        Confinement::Confined { hostagent } => (false, Some(*hostagent)),
        Confinement::Unconfined { hostagent, reason } => {
            warnings.push(format!("unconfined: {reason}"));
            (true, Some(*hostagent))
        }
        Confinement::Stopped => (false, None),
    };

    let mut forwards = serde_json::Map::new();
    for (key, role) in [("proxy", Role::FwdProxy), ("web", Role::FwdWeb)] {
        let st = crate::launchd::status(h.r, &label(Some(name), role))?;
        let sandboxed = st.pid.is_some_and(|p| h.probe.sandboxed(p));
        if st.pid.is_none() {
            warnings.push(format!("the {key} forward is not running"));
            down = true;
        } else if !sandboxed {
            warnings.push(format!("the {key} forward is not sandboxed"));
            down = true;
        }
        let mut v = agent(&st);
        v["sandboxed"] = json!(sandboxed);
        forwards.insert(key.into(), v);
    }

    let proxy_st = crate::launchd::status(h.r, &label(None, Role::Proxy))?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], cfg.ports.proxy));
    let listening = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok();
    let policy_error = read_events(&pd.network_log())?
        .into_iter()
        .rev()
        .find(|e| {
            matches!(
                e.kind,
                EventKind::Policy { .. } | EventKind::PolicyError { .. }
            )
        })
        .is_some_and(|e| matches!(e.kind, EventKind::PolicyError { .. }));
    if !listening {
        warnings.push(format!(
            "the proxy is not listening on 127.0.0.1:{}",
            cfg.ports.proxy
        ));
        down = true;
    }
    if policy_error {
        warnings.push(format!(
            "the proxy can't read the [network] policy (bunker net reload {name})"
        ));
        down = true;
    }

    let tunnel = match (&cfg.domain, &cfg.tunnel) {
        (Some(_), Some(_)) => {
            let st = crate::launchd::status(h.r, &label(Some(name), Role::Tunnel))?;
            let configured = pd.cloudflared_yml().exists();
            if configured && st.pid.is_none() {
                warnings.push("the tunnel is not running".into());
                down = true;
            }
            json!({"configured": configured, "loaded": st.loaded, "pid": st.pid})
        }
        _ => Value::Null,
    };

    let mut units = vec![
        "nginx".to_string(),
        format!("php{}-fpm", cfg.php),
        "mysql".to_string(),
    ];
    units.extend(
        cfg.workers
            .keys()
            .map(|w| format!("bunker-worker-{w}.service")),
    );
    let states: Vec<String> =
        if running {
            let out = h.r.run(&shell(h, &vm).args(["sudo", "bash", "-s"]).stdin(
                format!("systemctl is-active {} || true\n", units.join(" ")).into_bytes(),
            ))?;
            out.stdout_str()
                .lines()
                .map(|l| crate::output::printable(l).into_owned())
                .collect()
        } else {
            Vec::new()
        };
    let state_of = |i: usize| states.get(i).cloned().unwrap_or_else(|| "unknown".into());

    let release = if running {
        let out = h.r.run(
            &shell(h, &vm)
                .args(["sudo", "bash", "-s"])
                .stdin(releases_script(BASE).into_bytes()),
        )?;
        let text = out.stdout_str();
        let field = |k: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")))
                .unwrap_or("")
                .to_string()
        };
        let (cur, prev) = (field("current"), field("previous"));
        is_release_name(&cur).then(|| {
            json!({"current": cur, "sha": &cur[17..24], "previous": is_release_name(&prev).then_some(prev)})
        })
    } else {
        None
    };

    let mut workers = serde_json::Map::new();
    for (i, w) in cfg.workers.keys().enumerate() {
        workers.insert(w.clone(), json!(state_of(3 + i)));
    }
    if running {
        for (i, unit) in units.iter().enumerate() {
            let st = state_of(i);
            // Workers wait for the first deploy (ConditionPathExists); not running before it is fine.
            let is_worker = i >= 3;
            if st != "active" && (!is_worker || release.is_some()) {
                warnings.push(format!("{unit} is {st}"));
                down = true;
            }
        }
    }

    Ok(One {
        value: json!({
            "project": name,
            "vm": {"state": state, "confinement": conf.as_str(), "hostagent_pid": hostagent},
            "forwards": forwards,
            "proxy": {"loaded": proxy_st.loaded, "listening": listening, "policy": if policy_error { "error" } else { "ok" }},
            "tunnel": tunnel,
            "services": {"nginx": state_of(0), "php_fpm": state_of(1), "mysql": state_of(2), "workers": workers},
            "release": release,
            "warnings": warnings,
        }),
        unconfined,
        down,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testing::{CONFINED, FakeProbe, host, project};
    use crate::lima::confine::Confinement;
    use crate::run::{Output, RecordingRunner};

    fn runner() -> RecordingRunner {
        let r = RecordingRunner::new();
        r.respond(
            "limactl",
            &["list"],
            Output::ok("{\"name\":\"bunker-t\",\"status\":\"Running\",\"dir\":\"/x\"}\n"),
        );
        for _ in 0..10 {
            r.respond(
                "launchctl",
                &["print"],
                Output::ok("gui/501/x = {\n\tstate = running\n\tpid = 777\n}\n"),
            );
        }
        r.respond_with("limactl", &["shell"], |c| {
            let script =
                String::from_utf8_lossy(c.stdin.as_deref().unwrap_or_default()).into_owned();
            if script.contains("systemctl is-active") {
                Output::ok("active\nactive\nactive\nactive\n")
            } else {
                Output::ok("")
            }
        });
        r.respond(
            "limactl",
            &["shell"],
            Output::ok("current=20261116T000002Z-abc1234\nprevious=20261116T000001Z-abc0000\n"),
        );
        r
    }

    #[test]
    fn status_json_shape() {
        let d = tempfile::tempdir().unwrap();
        let r = runner();
        let h = host(&r, &CONFINED, d.path());
        let pd = project(&h, "t");
        std::fs::write(pd.cloudflared_yml(), "x").unwrap();
        let rep = status(&h, Some("t")).unwrap();
        let v = rep.json();
        for path in [
            "/project",
            "/vm/state",
            "/vm/confinement",
            "/vm/hostagent_pid",
            "/forwards/proxy/loaded",
            "/forwards/proxy/pid",
            "/forwards/proxy/sandboxed",
            "/forwards/web/loaded",
            "/forwards/web/pid",
            "/forwards/web/sandboxed",
            "/proxy/loaded",
            "/proxy/listening",
            "/proxy/policy",
            "/tunnel/configured",
            "/tunnel/loaded",
            "/tunnel/pid",
            "/services/nginx",
            "/services/php_fpm",
            "/services/mysql",
            "/services/workers/queue",
            "/release/current",
            "/release/sha",
            "/release/previous",
            "/warnings",
        ] {
            assert!(v.pointer(path).is_some(), "{path} missing from {v:#}");
        }
        assert_eq!(v["vm"]["confinement"], "confined");
        assert_eq!(v["services"]["nginx"], "active");
        assert_eq!(v["release"]["sha"], "abc1234");
        assert_eq!(v["forwards"]["web"]["sandboxed"], true);
        // One systemctl call for every guest service.
        let n = r
            .calls()
            .iter()
            .filter(|c| {
                String::from_utf8_lossy(c.stdin.as_deref().unwrap_or_default())
                    .contains("systemctl is-active")
            })
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn status_exit_5_when_unconfined() {
        let d = tempfile::tempdir().unwrap();
        let r = runner();
        let probe = FakeProbe(Confinement::Unconfined {
            hostagent: 9,
            reason: "not sandboxed".into(),
        });
        let h = host(&r, &probe, d.path());
        project(&h, "t");
        let rep = status(&h, Some("t")).unwrap();
        assert_eq!(rep.exit(), ExitCode::Unconfined);
        assert!(
            rep.json()["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w.as_str().unwrap().contains("not sandboxed"))
        );
    }

    #[test]
    fn status_of_all_projects_is_an_array() {
        let d = tempfile::tempdir().unwrap();
        let r = runner();
        let h = host(&r, &CONFINED, d.path());
        project(&h, "t");
        let v = status(&h, None).unwrap().json();
        assert!(
            v.as_array()
                .is_some_and(|a| a.len() == 1 && a[0]["project"] == "t"),
            "{v}"
        );
    }
}
