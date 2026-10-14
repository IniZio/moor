//! Tests for address classification and destination vetting.

use super::*;

#[test]
fn refuses_non_public_addresses() {
    let refused = [
        "0.0.0.0",
        "0.255.255.255",
        "10.0.0.0",
        "10.255.255.255",
        "100.64.0.0",
        "100.127.255.255",
        "127.0.0.1",
        "127.255.255.255",
        "169.254.169.254",
        "172.16.0.0",
        "172.31.255.255",
        "192.0.0.0",
        "192.0.0.255",
        "192.0.2.1",
        "192.88.99.1",
        "192.168.0.1",
        "192.168.255.255",
        "198.18.0.0",
        "198.19.255.255",
        "198.51.100.7",
        "203.0.113.9",
        "224.0.0.1",
        "239.255.255.255",
        "240.0.0.0",
        "255.255.255.255",
        "::",
        "::1",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "::127.0.0.1",
        "64:ff9b::7f00:1",
        "64:ff9b:1::1",
        "64:ff9b:1:ffff::1",
        "100::",
        "100::ffff",
        "2001::",
        "2001:1::1",
        "2001:1ff::1",
        "2001:db8::1",
        "2002:7f00:1::",
        "2002:a00:1::",
        "fc00::1",
        "fdff::1",
        "fe80::1",
        "febf::1",
        "fec0::1",
        "ff02::1",
    ];
    for text in refused {
        let ip: IpAddr = text.parse().unwrap();
        assert!(!is_public(ip), "{text} must be refused");
    }
}

#[test]
fn allows_public_addresses() {
    let allowed = [
        "1.1.1.1",
        "8.8.8.8",
        "9.255.255.255",
        "11.0.0.0",
        "100.63.255.255",
        "100.128.0.0",
        "126.255.255.255",
        "128.0.0.0",
        "169.253.255.255",
        "169.255.0.0",
        "172.15.255.255",
        "172.32.0.0",
        "192.0.1.1",
        "192.0.3.1",
        "192.88.98.1",
        "192.88.100.1",
        "192.167.255.255",
        "192.169.0.0",
        "198.17.255.255",
        "198.20.0.0",
        "198.51.99.1",
        "198.51.101.1",
        "203.0.112.1",
        "203.0.114.1",
        "223.255.255.255",
        "2606:4700::1111",
        "::ffff:8.8.8.8",
        "64:ff9b::808:808",
        "64:ff9b:2::1",
        "100:0:0:1::1",
        "2001:200::1",
        "2001:db9::1",
        "2002:808:808::",
        "2a00::1",
        "fbff::1",
        "fe7f::1",
        "2fff::1",
    ];
    for text in allowed {
        let ip: IpAddr = text.parse().unwrap();
        assert!(is_public(ip), "{text} must be allowed");
    }
}

#[test]
fn localhost_needs_exemption() {
    let port = 4433;
    assert!(Guard::default().resolve("localhost", port).is_err());
    let exempt = Guard::new(vec![SocketAddr::from(([127, 0, 0, 1], port))]);
    let got = exempt.resolve("localhost", port).unwrap();
    assert!(!got.is_empty());
    assert!(got.iter().all(|a| a.ip().is_loopback() && a.port() == port));
    let other = Guard::new(vec![SocketAddr::from(([127, 0, 0, 1], port + 1))]);
    assert!(other.resolve("localhost", port).is_err());
}

#[test]
fn exemption_is_loopback_only() {
    let guard = Guard::new(vec![SocketAddr::from(([10, 0, 0, 1], 80))]);
    assert!(guard.resolve("10.0.0.1", 80).is_err());
    assert!(guard.resolve("127.0.0.1", 80).is_err());
    let guard = Guard::new(vec![SocketAddr::from(([127, 0, 0, 1], 80))]);
    assert!(guard.resolve("10.0.0.1", 80).is_err());
    assert!(guard.resolve("::1", 80).is_ok());
}

#[test]
fn literals_are_vetted() {
    let guard = Guard::default();
    for host in ["127.0.0.1", "[::1]", "::1", "10.0.0.1", "169.254.169.254"] {
        assert!(
            matches!(guard.resolve(host, 80), Err(GuardError::NotPublic { .. })),
            "{host}"
        );
    }
    let got = guard.resolve("1.1.1.1", 443).unwrap();
    assert_eq!(got, vec![SocketAddr::from(([1, 1, 1, 1], 443))]);
    let got = guard.resolve("[2606:4700::1111]", 443).unwrap();
    assert_eq!(got.len(), 1);
}

#[test]
fn unresolvable_name_fails() {
    let err = Guard::default()
        .resolve("nonexistent.invalid", 443)
        .unwrap_err();
    assert!(matches!(err, GuardError::Unresolved { .. }), "{err}");
}

#[test]
fn messages_omit_addresses() {
    let err = Guard::default().resolve("10.0.0.1", 80).unwrap_err();
    assert!(!err.to_string().contains("10.0.0.1:"));
    let text = format!(
        "{:?}",
        Guard::new(vec![SocketAddr::from(([127, 0, 0, 1], 1))])
    );
    assert_eq!(text, "Guard { exempt: 1 }");
}
