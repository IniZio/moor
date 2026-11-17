#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::net::IpAddr;

use super::{err, ok};
use crate::config::{reachable, Served};

#[test]
fn the_machine_service_names_a_host_and_an_upstream() {
    let cfg = ok("machines: {service: {host: machine.moor.internal, upstream: '127.0.0.1:7700'}}");
    let service = cfg.machines.service.unwrap();
    assert_eq!(service.served, Served::Machine);
    assert!(service.serves("Machine.Moor.Internal"));
    assert!(!service.serves("machine.moor.internal.example.com"));
    assert_eq!(service.upstream.as_str(), "127.0.0.1:7700");
    assert_eq!(ok("").machines.service, None);
    assert_eq!(
        ok("machines: {service: {upstream: '127.0.0.1:7700'}}")
            .machines
            .service
            .unwrap()
            .host,
        "machine.moor.internal"
    );
}

#[test]
fn an_upstream_may_be_private_or_a_dns_name_looked_up_later() {
    for upstream in [
        "127.0.0.1:7700",
        "10.0.0.5:7700",
        "172.16.3.4:7700",
        "192.168.1.2:7700",
        "100.64.0.9:7700",
        "[::1]:7700",
        "[fd00::5]:7700",
        "localhost:7700",
        "moor-gate.moor.svc:8444",
    ] {
        let cfg = ok(&format!("moor_service: {{upstream: '{upstream}'}}"));
        assert_eq!(cfg.moor_service.unwrap().upstream.as_str(), upstream);
    }
}

#[test]
fn a_public_or_unspecified_upstream_is_refused_at_load() {
    for upstream in [
        "8.8.8.8:7700",
        "0.0.0.0:7700",
        "169.254.169.254:80",
        "[fe80::1]:7700",
        "[2001:4860::8888]:7700",
        "[::ffff:8.8.8.8]:7700",
    ] {
        let e = err(&format!(
            "machines: {{service: {{host: machine.moor.internal, upstream: '{upstream}'}}}}"
        ));
        assert_eq!(e.path, "machines.service.upstream", "{upstream}");
        assert!(
            e.reason.contains("not a loopback or private"),
            "{upstream}: {e:?}"
        );
    }
    for upstream in ["127.0.0.1", "moor.svc", "http://x:1", "x:0", "a/b:1"] {
        let e = err(&format!("moor_service: {{upstream: '{upstream}'}}"));
        assert_eq!(e.path, "moor_service.upstream", "{upstream}");
    }
}

#[test]
fn only_loopback_private_and_unique_local_addresses_are_reachable() {
    let table = [
        ("127.0.0.1", true),
        ("10.255.0.1", true),
        ("172.31.255.255", true),
        ("172.32.0.1", false),
        ("192.168.0.1", true),
        ("100.64.0.1", true),
        ("100.127.255.255", true),
        ("100.128.0.1", false),
        ("169.254.169.254", false),
        ("0.0.0.0", false),
        ("1.1.1.1", false),
        ("::1", true),
        ("fc00::1", true),
        ("fdff::1", true),
        ("fe80::1", false),
        ("::", false),
        ("2606:4700::1111", false),
        ("::ffff:10.0.0.1", true),
        ("::ffff:1.1.1.1", false),
    ];
    for (ip, want) in table {
        assert_eq!(reachable(ip.parse::<IpAddr>().unwrap()), want, "{ip}");
    }
}

#[test]
fn a_service_host_that_is_not_a_dns_name_is_refused() {
    for host in ["''", "'machine.moor.internal:80'", "'a/b'", "'-x.internal'"] {
        let e = err(&format!(
            "machines: {{service: {{host: {host}, upstream: '127.0.0.1:7700'}}}}"
        ));
        assert_eq!(e.path, "machines.service.host", "{host}");
    }
    let e = err("moor_service: {host: 'a/b', upstream: '127.0.0.1:8444'}");
    assert_eq!(e.path, "moor_service.host");
}

#[test]
fn the_machine_service_is_an_mcp_server_every_box_gets() {
    let cfg = ok("machines: {service: {host: machine.moor.internal, upstream: '127.0.0.1:7700'}}");
    let shared = cfg.shared_mcp();
    let machine = &shared["machine"];
    assert_eq!(
        machine.url.as_deref(),
        Some("http://machine.moor.internal/mcp")
    );
    assert_eq!(machine.kind.as_deref(), Some("http"));
    assert!(ok("").shared_mcp().is_empty());

    let own = ok(
        "mcp_servers: {machine: {url: 'https://mine.example.com/mcp'}}\n\
         machines: {service: {host: machine.moor.internal, upstream: '127.0.0.1:7700'}}",
    );
    assert_eq!(
        own.shared_mcp()["machine"].url.as_deref(),
        Some("https://mine.example.com/mcp"),
        "an operator entry of that name stays"
    );
}

#[test]
fn moor_service_is_an_mcp_server_every_box_gets_at_its_default_or_named_host() {
    let cfg = ok("moor_service: {upstream: '127.0.0.1:8444'}");
    let moor = &cfg.shared_mcp()["moor"];
    assert_eq!(moor.url.as_deref(), Some("http://moor.moor.internal/mcp"));
    assert_eq!(moor.kind.as_deref(), Some("http"));
    assert_eq!(cfg.served_hosts().count(), 1);

    let named = ok("moor_service: {host: gate.internal, upstream: '127.0.0.1:8444'}");
    assert_eq!(
        named.shared_mcp()["moor"].url.as_deref(),
        Some("http://gate.internal/mcp")
    );

    let own = ok(
        "mcp_servers: {moor: {url: 'https://mine.example.com/mcp'}}\n\
         moor_service: {upstream: '127.0.0.1:8444'}",
    );
    assert_eq!(
        own.shared_mcp()["moor"].url.as_deref(),
        Some("https://mine.example.com/mcp"),
        "an operator entry of that name stays"
    );
}

#[test]
fn no_entry_means_no_served_host_and_no_mcp_server() {
    let none = ok("");
    assert_eq!(none.served_hosts().count(), 0);
    assert!(!none.shared_mcp().contains_key("moor"));
    assert!(!none.shared_mcp().contains_key("machine"));
    let e = err("moor_service: {}");
    assert!(e.reason.contains("upstream"), "{e:?}");
}
