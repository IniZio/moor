//! Tests of which addresses count as public.
#![allow(clippy::unwrap_used, reason = "tests")]

use std::net::IpAddr;

use super::is_public;

/// Addresses that are not globally routable, one or more per kind.
const REFUSED: &[&str] = &[
    "0.0.0.0",
    "0.1.2.3",
    "10.0.0.1",
    "10.255.255.255",
    "100.64.0.1",
    "100.127.255.254",
    "127.0.0.1",
    "127.1.2.3",
    "169.254.169.254",
    "172.16.0.1",
    "172.31.255.255",
    "192.0.0.8",
    "192.0.2.1",
    "192.168.1.1",
    "198.18.0.1",
    "198.51.100.7",
    "203.0.113.9",
    "224.0.0.1",
    "239.255.255.250",
    "240.0.0.1",
    "255.255.255.255",
    "::",
    "::1",
    "::127.0.0.1",
    "::ffff:127.0.0.1",
    "::ffff:10.0.0.1",
    "::ffff:169.254.169.254",
    "64:ff9b::a00:1",
    "64:ff9b:1::1",
    "100::1",
    "2001::1",
    "2001:db8::1",
    "2002:7f00:1::1",
    "3fff::1",
    "fc00::1",
    "fd12:3456::1",
    "fe80::1",
    "febf::1",
    "fec0::1",
    "ff02::1",
];

/// Globally routable addresses, including neighbours of refused blocks.
const ACCEPTED: &[&str] = &[
    "1.1.1.1",
    "8.8.8.8",
    "9.255.255.255",
    "11.0.0.1",
    "100.63.255.255",
    "100.128.0.1",
    "126.255.255.255",
    "128.0.0.1",
    "169.253.255.255",
    "172.15.255.255",
    "172.32.0.1",
    "192.169.0.1",
    "223.255.255.255",
    "2606:4700::1111",
    "2001:4860:4860::8888",
    "2a00:1450:4001::1",
    "::ffff:1.1.1.1",
    "64:ff9b::101:101",
];

#[test]
fn refuses_every_address_that_is_not_globally_routable() {
    for text in REFUSED {
        let ip: IpAddr = text.parse().unwrap();
        assert!(!is_public(ip), "{text} should be refused");
    }
}

#[test]
fn accepts_public_addresses() {
    for text in ACCEPTED {
        let ip: IpAddr = text.parse().unwrap();
        assert!(is_public(ip), "{text} should be accepted");
    }
}
