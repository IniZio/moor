//! Keeping POSTs off addresses that are not on the public internet, since any user may set
//! a webhook's URL and the front runs next to the controller's own ports.
//!
//! The check lives in ureq's resolver, so the addresses checked are the addresses connected
//! to, every redirect is checked the same way, and TLS still verifies the URL's host name.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::NextTimeout;

/// The reason reported for a delivery whose host resolved to an address that is not public.
pub const NOT_PUBLIC: &str = "refused: address not public";

/// Which addresses a POST may connect to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Only globally routable addresses.
    Public,
    /// Any address, for local development and tests with a loopback receiver.
    Any,
}

/// IPv4 blocks that are not globally routable, as (network, prefix length).
const V4_BLOCKED: [(Ipv4Addr, u8); 15] = [
    (Ipv4Addr::UNSPECIFIED, 8),
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(127, 0, 0, 0), 8),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(192, 0, 2, 0), 24),
    (Ipv4Addr::new(192, 88, 99, 0), 24),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(198, 51, 100, 0), 24),
    (Ipv4Addr::new(203, 0, 113, 0), 24),
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

/// IPv6 blocks that are not globally routable, as (network, prefix length). IPv4-mapped and
/// NAT64 addresses are judged by the IPv4 address they carry instead.
const V6_BLOCKED: [(Ipv6Addr, u8); 11] = [
    (Ipv6Addr::UNSPECIFIED, 96),
    (Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0), 48),
    (Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0), 64),
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
    (Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
    (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10),
    (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
];

/// The NAT64 well-known prefix, `64:ff9b::/96`.
const NAT64: (Ipv6Addr, u8) = (Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0), 96);

/// Whether the top `len` bits of `a` and `net` agree.
fn same_prefix(a: u128, net: u128, len: u8, width: u32) -> bool {
    (a ^ net).checked_shr(width - u32::from(len)).unwrap_or(0) == 0
}

/// Whether `ip` is globally routable.
#[must_use]
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => !V4_BLOCKED
            .iter()
            .any(|&(net, len)| same_prefix(u32::from(a).into(), u32::from(net).into(), len, 32)),
        IpAddr::V6(a) => {
            let bits = u128::from(a);
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            if same_prefix(bits, u128::from(NAT64.0), NAT64.1, 128) {
                let low = u32::try_from(bits & u128::from(u32::MAX)).unwrap_or(0);
                return is_public(IpAddr::V4(Ipv4Addr::from(low)));
            }
            !V6_BLOCKED
                .iter()
                .any(|&(net, len)| same_prefix(bits, u128::from(net), len, 128))
        }
    }
}

/// The error a [`PublicOnly`] lookup fails with.
#[derive(Debug)]
pub(crate) struct NotPublic;

impl fmt::Display for NotPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(NOT_PUBLIC)
    }
}

impl std::error::Error for NotPublic {}

/// A resolver that fails when any address of a host is not public, and otherwise hands
/// ureq exactly the addresses it checked.
#[derive(Debug, Default)]
pub(crate) struct PublicOnly(DefaultResolver);

impl Resolver for PublicOnly {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addrs = self.0.resolve(uri, config, timeout)?;
        let operators_proxy = config.proxy().is_some_and(|p| p.uri() == uri);
        if operators_proxy || addrs.iter().all(|a| is_public(a.ip())) {
            Ok(addrs)
        } else {
            Err(ureq::Error::Other(Box::new(NotPublic)))
        }
    }
}

#[cfg(test)]
mod tests;
