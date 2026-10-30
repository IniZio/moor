#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::net::SocketAddr;

use super::{err, ok};

#[test]
fn the_machine_service_names_a_host_and_a_loopback_upstream() {
    let cfg = ok("machines: {service: {host: machine.moor.internal, upstream: '127.0.0.1:7700'}}");
    let service = cfg.machines.service.unwrap();
    assert!(service.serves("Machine.Moor.Internal"));
    assert!(!service.serves("machine.moor.internal.example.com"));
    assert_eq!(service.upstream, SocketAddr::from(([127, 0, 0, 1], 7700)));
    assert_eq!(ok("").machines.service, None);
}

#[test]
fn an_upstream_off_loopback_is_refused_at_load() {
    for upstream in [
        "10.0.0.5:7700",
        "0.0.0.0:7700",
        "localhost:7700",
        "127.0.0.1",
    ] {
        let e = err(&format!(
            "machines: {{service: {{host: machine.moor.internal, upstream: '{upstream}'}}}}"
        ));
        assert_eq!(e.path, "machines.service.upstream", "{upstream}");
        assert!(
            e.reason.contains("not a loopback address"),
            "{upstream}: {e:?}"
        );
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
