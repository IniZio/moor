//! Egress destination guard.
//!
//! A host name the Box may reach can resolve to loopback, private or
//! link-local space, which would let the Box use moor's proxy to reach
//! services on the user's machine or network. The guard resolves a
//! destination once, refuses it unless every answer is a public unicast
//! address, and returns exactly the vetted addresses for the caller to dial.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

/// IPv4 ranges that are not globally routable, as (network, prefix length).
const V4_NON_PUBLIC: [(u32, u32); 15] = [
    (0x0000_0000, 8),  // 0.0.0.0/8 this network
    (0x0A00_0000, 8),  // 10/8 private
    (0x6440_0000, 10), // 100.64/10 shared address space
    (0x7F00_0000, 8),  // 127/8 loopback
    (0xA9FE_0000, 16), // 169.254/16 link-local
    (0xAC10_0000, 12), // 172.16/12 private
    (0xC000_0000, 24), // 192.0.0/24 protocol assignments
    (0xC000_0200, 24), // 192.0.2/24 documentation
    (0xC058_6300, 24), // 192.88.99/24 6to4 relay anycast
    (0xC0A8_0000, 16), // 192.168/16 private
    (0xC612_0000, 15), // 198.18/15 benchmarking
    (0xC633_6400, 24), // 198.51.100/24 documentation
    (0xCB00_7100, 24), // 203.0.113/24 documentation
    (0xE000_0000, 4),  // 224/4 multicast
    (0xF000_0000, 4),  // 240/4 reserved and broadcast
];

/// IPv6 ranges that are not globally routable, as (network, prefix length).
const V6_NON_PUBLIC: [(u128, u32); 8] = [
    (0x0064_ff9b_0001 << 80, 48), // 64:ff9b:1::/48 local-use translation
    (0x0100 << 112, 64),          // 100::/64 discard-only
    (0x2001 << 112, 23),          // 2001::/23 protocol assignments, Teredo
    (0x2001_0db8 << 96, 32),      // 2001:db8::/32 documentation
    (0xfc00 << 112, 7),           // fc00::/7 unique local
    (0xfe80 << 112, 10),          // fe80::/10 link-local
    (0xfec0 << 112, 10),          // fec0::/10 site-local
    (0xff00 << 112, 8),           // ff00::/8 multicast
];

/// Whether `ip` lies in the `prefix`-bit network starting at `net`.
fn in_v4(ip: u32, net: u32, prefix: u32) -> bool {
    let mask = u32::MAX.checked_shl(32 - prefix).unwrap_or(0);
    ip & mask == net
}

/// Whether `ip` lies in the `prefix`-bit network starting at `net`.
fn in_v6(ip: u128, net: u128, prefix: u32) -> bool {
    let mask = u128::MAX.checked_shl(128 - prefix).unwrap_or(0);
    ip & mask == net
}

/// Whether the IPv4 address is globally routable unicast.
fn v4_public(ip: Ipv4Addr) -> bool {
    let bits = u32::from(ip);
    !V4_NON_PUBLIC
        .iter()
        .any(|&(net, prefix)| in_v4(bits, net, prefix))
}

/// Whether the IPv6 address is globally routable unicast.
fn v6_public(ip: Ipv6Addr) -> bool {
    let bits = u128::from(ip);
    // The low 32 bits are an IPv4 address for these forms; judge that address.
    let low = Ipv4Addr::from(u32::try_from(bits & 0xFFFF_FFFF).unwrap_or(0));
    if in_v6(bits, 0, 96) || in_v6(bits, 0xffff << 32, 96) || in_v6(bits, 0x0064_ff9b << 96, 96) {
        return v4_public(low);
    }
    if in_v6(bits, 0x2002 << 112, 16) {
        let embedded = u32::try_from((bits >> 80) & 0xFFFF_FFFF).unwrap_or(0);
        return v4_public(Ipv4Addr::from(embedded));
    }
    !V6_NON_PUBLIC
        .iter()
        .any(|&(net, prefix)| in_v6(bits, net, prefix))
}

/// Whether `ip` is a globally routable unicast address moor may dial.
pub(crate) fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4_public(v4),
        IpAddr::V6(v6) => v6_public(v6),
    }
}

/// Why a destination was refused.
#[derive(Debug, thiserror::Error)]
pub(crate) enum GuardError {
    /// The host name did not resolve to any address.
    #[error("cannot resolve {host}:{port}")]
    Unresolved {
        /// Host that failed to resolve.
        host: String,
        /// Port that was requested.
        port: u16,
    },
    /// At least one resolved address is not a public address.
    #[error("{host} resolves to a non-public address")]
    NotPublic {
        /// Host that was refused.
        host: String,
    },
}

/// Resolves destinations and refuses any that is not public.
#[derive(Clone, Default)]
pub(crate) struct Guard {
    /// Loopback addresses tests may reach.
    exempt: Vec<SocketAddr>,
}

impl fmt::Debug for Guard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Guard")
            .field("exempt", &self.exempt.len())
            .finish()
    }
}

impl Guard {
    /// `exempt`: loopback addresses tests may reach; empty in production.
    pub(crate) const fn new(exempt: Vec<SocketAddr>) -> Self {
        Self { exempt }
    }

    /// Whether `addr` is loopback and shares a port with a loopback exemption.
    fn is_exempt(&self, addr: SocketAddr) -> bool {
        addr.ip().is_loopback()
            && self
                .exempt
                .iter()
                .any(|e| e.ip().is_loopback() && e.port() == addr.port())
    }

    /// Resolves `host:port` once and vets every answer.
    ///
    /// IP literals and bracketed IPv6 are accepted. Fails when nothing
    /// resolves or when any answer is neither public nor exempt. The result
    /// holds the vetted addresses in resolver order; the caller must dial
    /// those and never resolve again.
    pub(crate) fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, GuardError> {
        let bare = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        let unresolved = || GuardError::Unresolved {
            host: host.to_owned(),
            port,
        };
        let addrs: Vec<SocketAddr> = (bare, port)
            .to_socket_addrs()
            .map_err(|_| unresolved())?
            .collect();
        if addrs.is_empty() {
            return Err(unresolved());
        }
        if addrs
            .iter()
            .any(|a| !is_public(a.ip()) && !self.is_exempt(*a))
        {
            return Err(GuardError::NotPublic {
                host: host.to_owned(),
            });
        }
        Ok(addrs)
    }
}

#[cfg(test)]
mod tests;
