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

/// IPv6 is public only inside global unicast `2000::/3` or the well-known NAT64 prefix with a
/// public IPv4 inside. Everything else fails closed, including Teredo, the documentation ranges and
/// the local-use NAT64 prefix; 6to4 is judged by the IPv4 address it embeds.
fn v6_internal(ip: Ipv6Addr, own: &[IpAddr]) -> bool {
    let seg = ip.segments();
    let embedded = |hi: u16, lo: u16| {
        let v4 = Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
        own.contains(&IpAddr::V4(v4)) || v4_internal(v4)
    };
    // ::ffff:a.b.c.d (mapped) and ::a.b.c.d (compatible); :: and ::1 land here too.
    if seg[..5] == [0; 5] && (seg[5] == 0xffff || seg[5] == 0) {
        return ip.is_unspecified() || ip.is_loopback() || embedded(seg[6], seg[7]);
    }
    // Well-known NAT64 prefix 64:ff9b::/96.
    if seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return embedded(seg[6], seg[7]);
    }
    if (seg[0] & 0xe000) != 0x2000 {
        return true;
    }
    // 2001::/32 Teredo, 2001:db8::/32 and 3fff::/20 documentation.
    if (seg[0] == 0x2001 && (seg[1] == 0 || seg[1] == 0x0db8)) || (seg[0] & 0xfff0) == 0x3ff0 {
        return true;
    }
    // 2002::/16 6to4 carries an IPv4 address in bits 16-47.
    if seg[0] == 0x2002 {
        return embedded(seg[1], seg[2]);
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
            "64:ff9b:1::808:808",
            "2001:0:4136:e378::1",
            "2001:db8::1",
            "2002:a00:1::1",
            "2002:7f00:1::1",
            "fec0::1",
            "100::1",
            "3fff::1",
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
            "2002:101:101::1",
            "2a00:1450:4001::200e",
        ] {
            assert!(
                !is_internal(s.parse().unwrap(), &own),
                "{s} should be public"
            );
        }
    }
}
