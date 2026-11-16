//! Host ports: one SSH, one proxy and one web port per project, from fixed ranges.
use crate::config::Ports;
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use std::ops::RangeInclusive;

pub const SSH_RANGE: RangeInclusive<u16> = 60100..=60199;
pub const PROXY_RANGE: RangeInclusive<u16> = 18800..=18899;
pub const WEB_RANGE: RangeInclusive<u16> = 18900..=18999;

/// The lowest port in each range that no other project holds and nothing is listening on.
pub fn allocate_ports(taken: &[Ports], is_free: &dyn Fn(u16) -> bool) -> Result<Ports> {
    let pick = |range: RangeInclusive<u16>, what: &str, held: &dyn Fn(&Ports) -> u16| {
        range
            .clone()
            .find(|p| !taken.iter().any(|t| held(t) == *p) && is_free(*p))
            .ok_or_else(|| {
                BunkerError::new(
                    "ports_exhausted",
                    ExitCode::Failure,
                    format!("no free {what} port in {}-{}", range.start(), range.end()),
                )
            })
    };
    Ok(Ports {
        ssh: pick(SSH_RANGE, "SSH", &|t| t.ssh)?,
        proxy: pick(PROXY_RANGE, "proxy", &|t| t.proxy)?,
        web: pick(WEB_RANGE, "web", &|t| t.web)?,
    })
}

/// True when nothing listens on `127.0.0.1:port` right now.
pub fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_allocation_skips_taken_and_busy() {
        let taken = [Ports {
            ssh: 60100,
            proxy: 18800,
            web: 18900,
        }];
        let p = allocate_ports(&taken, &|p| p != 60101 && p != 18801).unwrap();
        assert_eq!(
            p,
            Ports {
                ssh: 60102,
                proxy: 18802,
                web: 18901
            }
        );
    }

    #[test]
    fn port_allocation_errors_when_a_range_is_full() {
        let e = allocate_ports(&[], &|p| !SSH_RANGE.contains(&p)).unwrap_err();
        assert_eq!(e.code, "ports_exhausted");
    }
}
