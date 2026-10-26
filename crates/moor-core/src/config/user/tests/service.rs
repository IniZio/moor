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
