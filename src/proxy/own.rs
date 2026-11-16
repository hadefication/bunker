//! The Mac's own interface addresses, which the proxy refuses like any internal range: a public
//! IP on the Mac would otherwise reach host services bound to it.
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Every IPv4 and IPv6 address on every interface, from `getifaddrs(3)`; `None` on failure.
pub fn own_addresses() -> Option<Vec<IpAddr>> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we own until freeifaddrs; we only read it.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return None;
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
    Some(out)
}

type Lookup = Box<dyn Fn() -> Option<Vec<IpAddr>> + Send + Sync>;

/// Looks the addresses up on every call and keeps the last good list when a lookup fails, so a
/// getifaddrs failure never makes the Mac's own addresses look public.
pub struct OwnCache {
    lookup: Lookup,
    last: std::sync::Mutex<Vec<IpAddr>>,
}

impl OwnCache {
    /// `None` when the first lookup fails: the proxy must not start without a list.
    pub fn new(lookup: impl Fn() -> Option<Vec<IpAddr>> + Send + Sync + 'static) -> Option<Self> {
        let first = lookup()?;
        Some(Self {
            lookup: Box::new(lookup),
            last: std::sync::Mutex::new(first),
        })
    }

    pub fn get(&self) -> Vec<IpAddr> {
        let mut last = self.last.lock().unwrap();
        if let Some(now) = (self.lookup)() {
            *last = now;
        }
        last.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_addresses_include_loopback() {
        let a = own_addresses().unwrap();
        assert!(a.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST)), "{a:?}");
        assert!(a.contains(&IpAddr::V6(Ipv6Addr::LOCALHOST)), "{a:?}");
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn failed_lookup_keeps_last_good_list() {
        let lan: IpAddr = "192.0.2.10".parse().unwrap();
        let answers = std::sync::Mutex::new(vec![Some(vec![lan]), Some(vec![lan]), None]);
        let own = OwnCache::new(move || answers.lock().unwrap().remove(0)).unwrap();
        assert_eq!(own.get(), vec![lan]);
        assert_eq!(
            own.get(),
            vec![lan],
            "a failed getifaddrs must not empty the list"
        );
        assert!(
            OwnCache::new(|| None).is_none(),
            "no first list means no proxy"
        );
    }
}
