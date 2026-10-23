//! The addresses no front may reach through its egress proxy: the private, loopback,
//! link-local, CGNAT, multicast and reserved ranges, and every address of every interface the
//! host had when the front started, public ones included.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The IPv4 ranges, as network and prefix length.
const RANGES_V4: [(Ipv4Addr, u32); 11] = [
    (Ipv4Addr::UNSPECIFIED, 8),
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(127, 0, 0, 0), 8),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

/// The IPv6 ranges, as network and prefix length.
const RANGES_V6: [(Ipv6Addr, u32); 5] = [
    (Ipv6Addr::UNSPECIFIED, 128),
    (Ipv6Addr::LOCALHOST, 128),
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
];

/// Why an address is denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reason {
    /// It lies in a private, loopback, link-local, CGNAT, multicast or reserved range.
    Range,
    /// An interface of the host holds it.
    Host,
}

impl Reason {
    /// The word a log line gives it.
    pub(super) const fn word(self) -> &'static str {
        match self {
            Self::Range => "private-range",
            Self::Host => "host-address",
        }
    }
}

/// The denied addresses.
pub(super) struct Denied {
    /// The host's addresses.
    host: Vec<IpAddr>,
}

impl Denied {
    /// The ranges and `host`, the host's own addresses.
    pub(super) const fn new(host: Vec<IpAddr>) -> Self {
        Self { host }
    }

    /// Why `addr` is denied; none when a front may reach it. An IPv4-mapped IPv6 address is
    /// judged as the IPv4 address it carries.
    pub(super) fn why(&self, addr: IpAddr) -> Option<Reason> {
        let addr = match addr {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(addr, IpAddr::V4),
            v4 @ IpAddr::V4(_) => v4,
        };
        let in_range = match addr {
            IpAddr::V4(v4) => RANGES_V4
                .iter()
                .any(|(net, len)| within(u32::from(v4).into(), u32::from(*net).into(), *len, 32)),
            IpAddr::V6(v6) => RANGES_V6
                .iter()
                .any(|(net, len)| within(u128::from(v6), u128::from(*net), *len, 128)),
        };
        if in_range {
            return Some(Reason::Range);
        }
        self.host.contains(&addr).then_some(Reason::Host)
    }
}

/// Whether `addr` lies in `net/len`, both `bits` wide.
const fn within(addr: u128, net: u128, len: u32, bits: u32) -> bool {
    let shift = bits - len;
    if shift >= 128 {
        return true;
    }
    (addr >> shift) == (net >> shift)
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use super::{Denied, Reason};

    fn why(denied: &Denied, addr: &str) -> Option<Reason> {
        denied.why(addr.parse::<IpAddr>().unwrap())
    }

    #[test]
    fn each_range_and_host_address_is_denied_and_the_public_internet_is_not() {
        let denied = Denied::new(vec!["203.0.113.5".parse().unwrap()]);
        for addr in [
            "0.1.2.3",
            "10.235.225.1",
            "100.64.0.156",
            "127.0.0.1",
            "169.254.0.53",
            "172.17.0.1",
            "192.0.0.8",
            "192.168.0.103",
            "198.19.255.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd7a:115c:a1e0::9d",
            "fe80::1",
            "ff02::1",
            "::ffff:10.0.0.1",
        ] {
            assert_eq!(why(&denied, addr), Some(Reason::Range), "{addr}");
        }
        assert_eq!(why(&denied, "203.0.113.5"), Some(Reason::Host));
        assert_eq!(why(&denied, "::ffff:203.0.113.5"), Some(Reason::Host));
        for addr in [
            "1.1.1.1",
            "100.128.0.1",
            "172.32.0.1",
            "198.51.100.7",
            "2606:4700::1111",
        ] {
            assert_eq!(why(&denied, addr), None, "{addr}");
        }
    }
}
