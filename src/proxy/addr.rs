//! Which addresses count as internal: never reachable through the proxy.
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// True for any address a guest must never reach through the proxy: loopback, private, link-local,
/// CGNAT, multicast, broadcast, unspecified, `0.0.0.0/8`, `192.0.0.0/24`, IPv6 ULA and link-local,
/// IPv4-mapped, IPv4-compatible and NAT64 forms of an internal IPv4, and the Mac's own addresses.
pub fn is_internal(ip: IpAddr, own: &[IpAddr]) -> bool {
    if own.contains(&ip) {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => v4_internal(v4),
        IpAddr::V6(v6) => v6_internal(v6, own),
    }
}

fn v4_internal(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || a >= 240
}

fn v6_internal(ip: Ipv6Addr, own: &[IpAddr]) -> bool {
    let seg = ip.segments();
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    if (seg[0] & 0xfe00) == 0xfc00 || (seg[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    let embedded = |hi: u16, lo: u16| {
        let v4 = Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
        own.contains(&IpAddr::V4(v4)) || v4_internal(v4)
    };
    // ::ffff:a.b.c.d (mapped) and ::a.b.c.d (compatible, deprecated but still routable by some stacks)
    if seg[..5] == [0; 5] && (seg[5] == 0xffff || seg[5] == 0) {
        return embedded(seg[6], seg[7]);
    }
    // NAT64 well-known prefix 64:ff9b::/96
    if seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return embedded(seg[6], seg[7]);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_ranges_table() {
        let own: Vec<IpAddr> = vec!["203.0.113.7".parse().unwrap()];
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.254.114",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.255",
            "0.0.0.0",
            "0.1.2.3",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.0.8",
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "ff02::1",
            "::ffff:10.0.0.1",
            "::10.0.0.1",
            "64:ff9b::a00:1",
            "203.0.113.7",
        ] {
            assert!(
                is_internal(s.parse().unwrap(), &own),
                "{s} should be internal"
            );
        }
        for s in [
            "93.184.215.14",
            "1.1.1.1",
            "100.128.0.1",
            "172.32.0.1",
            "2606:4700:4700::1111",
            "64:ff9b::101:101",
            "::ffff:1.1.1.1",
        ] {
            assert!(
                !is_internal(s.parse().unwrap(), &own),
                "{s} should be public"
            );
        }
    }
}
