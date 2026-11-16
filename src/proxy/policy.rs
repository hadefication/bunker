//! Allow or block one request: deny wins; exact hosts or `*.domain` (subdomains only); a pattern
//! without a port allows 80 and 443 only.
use crate::config::Network;
use crate::net::edit::ListName;

pub const BUILTIN_BUILD: &[&str] = &[
    "ports.ubuntu.com",
    "ppa.launchpadcontent.net",
    "keyserver.ubuntu.com",
    "repo.mysql.com",
    "deb.nodesource.com",
    "getcomposer.org",
    "repo.packagist.org",
    "packagist.org",
    "github.com",
    "api.github.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
    "registry.npmjs.org",
];

pub const MAINTENANCE: &[&str] = &[
    "ports.ubuntu.com",
    "ppa.launchpadcontent.net",
    "repo.mysql.com",
    "deb.nodesource.com",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Phases {
    pub build: bool,
    pub maintenance: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    host: String,
    wildcard: bool,
    port: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    pub allow: Vec<Pattern>,
    pub build: Vec<Pattern>,
    pub deny: Vec<Pattern>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    List(ListName),
    Builtin,
    Maintenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Listed(Source),
    Denied,
    NotListed,
    Port,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub verdict: Verdict,
    pub reason: Reason,
}

fn norm(h: &str) -> String {
    crate::net::edit::normalize_host(h)
}

impl Pattern {
    /// Parses a validated entry. IPv6 literals and malformed ports yield `None`.
    pub fn parse(s: &str) -> Option<Pattern> {
        let (host, port) = match s.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => {
                (h, Some(p.parse::<u16>().ok().filter(|p| *p > 0)?))
            }
            Some(_) => return None,
            None => (s, None),
        };
        let host = norm(host);
        let (host, wildcard) = match host.strip_prefix("*.") {
            Some(rest) => (rest.to_string(), true),
            None => (host, false),
        };
        (!host.is_empty()).then_some(Pattern {
            host,
            wildcard,
            port,
        })
    }

    fn host_matches(&self, host: &str) -> bool {
        if self.wildcard {
            host.strip_suffix(self.host.as_str())
                .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
        } else {
            host == self.host
        }
    }

    fn port_matches(&self, port: u16) -> bool {
        match self.port {
            Some(p) => p == port,
            None => port == 80 || port == 443,
        }
    }
}

impl Policy {
    pub fn from_network(n: &Network) -> Policy {
        let parse = |l: &[String]| l.iter().filter_map(|s| Pattern::parse(s)).collect();
        Policy {
            allow: parse(&n.allow),
            build: parse(&n.build),
            deny: parse(&n.deny),
        }
    }

    pub fn decide(&self, host: &str, port: u16, phases: Phases) -> Decision {
        let host = norm(host);
        let block = |reason| Decision {
            verdict: Verdict::Block,
            reason,
        };
        // A deny entry without a port blocks the host on every port.
        if self
            .deny
            .iter()
            .any(|p| p.host_matches(&host) && (p.port.is_none() || p.port == Some(port)))
        {
            return block(Reason::Denied);
        }
        let builtin = |list: &[&str]| -> Vec<Pattern> {
            list.iter().filter_map(|s| Pattern::parse(s)).collect()
        };
        let mut candidates: Vec<(Source, Vec<Pattern>)> =
            vec![(Source::List(ListName::Allow), self.allow.clone())];
        if phases.build {
            candidates.push((Source::List(ListName::Build), self.build.clone()));
            candidates.push((Source::Builtin, builtin(BUILTIN_BUILD)));
        }
        if phases.maintenance {
            candidates.push((Source::Maintenance, builtin(MAINTENANCE)));
        }
        let mut host_listed = false;
        for (source, patterns) in &candidates {
            for p in patterns.iter().filter(|p| p.host_matches(&host)) {
                if p.port_matches(port) {
                    return Decision {
                        verdict: Verdict::Allow,
                        reason: Reason::Listed(*source),
                    };
                }
                host_listed = true;
            }
        }
        block(if host_listed {
            Reason::Port
        } else {
            Reason::NotListed
        })
    }
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Block => "block",
        }
    }
}

impl Reason {
    /// The `reason` field in network.jsonl.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Listed(Source::List(l)) => l.key(),
            Reason::Listed(Source::Builtin) => "builtin",
            Reason::Listed(Source::Maintenance) => "maintenance",
            Reason::Denied => "denied",
            Reason::NotListed => "not_listed",
            Reason::Port => "port",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(allow: &[&str], build: &[&str], deny: &[&str]) -> Policy {
        let v = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
        Policy::from_network(&Network {
            allow: v(allow),
            build: v(build),
            deny: v(deny),
        })
    }
    const RUN: Phases = Phases {
        build: false,
        maintenance: false,
    };
    const BUILD: Phases = Phases {
        build: true,
        maintenance: false,
    };
    const MAINT: Phases = Phases {
        build: false,
        maintenance: true,
    };

    fn verdict(p: &Policy, host: &str, port: u16, ph: Phases) -> Verdict {
        p.decide(host, port, ph).verdict
    }

    #[test]
    fn deny_wins_over_allow() {
        let p = policy(&["*.github.com"], &[], &["gist.github.com"]);
        assert_eq!(
            p.decide("gist.github.com", 443, RUN),
            Decision {
                verdict: Verdict::Block,
                reason: Reason::Denied
            }
        );
        assert_eq!(
            p.decide("api.github.com", 443, RUN).reason,
            Reason::Listed(Source::List(ListName::Allow))
        );
        assert_eq!(
            policy(&[], &[], &["github.com"])
                .decide("github.com", 443, BUILD)
                .reason,
            Reason::Denied,
            "deny beats builtin"
        );
    }

    #[test]
    fn wildcard_matches_subdomains_not_apex() {
        let p = policy(&["*.example.com"], &[], &[]);
        assert_eq!(verdict(&p, "a.example.com", 443, RUN), Verdict::Allow);
        assert_eq!(verdict(&p, "a.b.example.com", 443, RUN), Verdict::Allow);
        assert_eq!(verdict(&p, "example.com", 443, RUN), Verdict::Block);
        assert_eq!(verdict(&p, "badexample.com", 443, RUN), Verdict::Block);
    }

    #[test]
    fn build_list_needs_build_phase() {
        let p = policy(&[], &["cdn.example.com"], &[]);
        assert_eq!(
            p.decide("cdn.example.com", 443, RUN).reason,
            Reason::NotListed
        );
        assert_eq!(
            p.decide("cdn.example.com", 443, BUILD).reason,
            Reason::Listed(Source::List(ListName::Build))
        );
        assert_eq!(
            p.decide("repo.packagist.org", 443, RUN).verdict,
            Verdict::Block
        );
        assert_eq!(
            p.decide("repo.packagist.org", 443, BUILD).reason,
            Reason::Listed(Source::Builtin)
        );
    }

    #[test]
    fn maintenance_list_needs_maintenance_phase() {
        let p = policy(&[], &[], &[]);
        assert_eq!(verdict(&p, "repo.mysql.com", 80, RUN), Verdict::Block);
        assert_eq!(
            p.decide("repo.mysql.com", 80, MAINT).reason,
            Reason::Listed(Source::Maintenance)
        );
        assert_eq!(
            verdict(&p, "registry.npmjs.org", 443, MAINT),
            Verdict::Block,
            "build-only host"
        );
    }

    #[test]
    fn ports_other_than_80_443_need_explicit_entry() {
        let p = policy(&["example.com"], &[], &[]);
        assert_eq!(verdict(&p, "example.com", 80, RUN), Verdict::Allow);
        assert_eq!(
            p.decide("example.com", 8443, RUN),
            Decision {
                verdict: Verdict::Block,
                reason: Reason::Port
            }
        );
        let p = policy(&["example.com:8443"], &[], &[]);
        assert_eq!(verdict(&p, "example.com", 8443, RUN), Verdict::Allow);
        assert_eq!(p.decide("example.com", 443, RUN).reason, Reason::Port);
    }

    #[test]
    fn host_matching_ignores_case_and_trailing_dot() {
        let p = policy(&["Api.Example.com."], &[], &[]);
        assert_eq!(verdict(&p, "API.example.COM.", 443, RUN), Verdict::Allow);
        assert_eq!(verdict(&p, "api.example.com", 443, RUN), Verdict::Allow);
    }
}
