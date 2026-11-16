//! The Mac's own interface addresses, which the proxy refuses like any internal range: a public
//! IP on the Mac would otherwise reach host services bound to it.
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Every IPv4 and IPv6 address on every interface, from `getifaddrs(3)`. Empty on failure.
pub fn own_addresses() -> Vec<IpAddr> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we own until freeifaddrs; we only read it.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return vec![];
    }
    let mut out = Vec::new();
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: `cur` is a node of the list getifaddrs returned and has not been freed.
        let ifa = unsafe { &*cur };
        if !ifa.ifa_addr.is_null() {
            // SAFETY: ifa_addr points at a sockaddr whose family says which struct it is.
            match unsafe { (*ifa.ifa_addr).sa_family } as i32 {
                libc::AF_INET => {
                    let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                    out.push(IpAddr::V4(Ipv4Addr::from(u32::from_be(
                        sin.sin_addr.s_addr,
                    ))));
                }
                libc::AF_INET6 => {
                    let sin6 = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                    out.push(IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr)));
                }
                _ => {}
            }
        }
        cur = ifa.ifa_next;
    }
    // SAFETY: `head` came from a successful getifaddrs and is freed exactly once.
    unsafe { libc::freeifaddrs(head) };
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_addresses_include_loopback() {
        let a = own_addresses();
        assert!(a.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST)), "{a:?}");
        assert!(a.contains(&IpAddr::V6(Ipv6Addr::LOCALHOST)), "{a:?}");
    }
}
