use clap::{CommandFactory as _, Parser as _};

use super::Args;

fn parse(extra: &[&str]) -> Result<Args, clap::Error> {
    let base = ["moor-machine-runner", "--state-dir", "/s"];
    Args::try_parse_from(base.iter().chain(extra))
}

#[test]
fn the_flags_are_consistent() {
    Args::command().debug_assert();
}

#[test]
fn a_tls_controller_takes_the_place_of_the_socket_and_needs_its_ca_client_cert_key_and_address() {
    let tls = [
        "--controller",
        "tls://ctl:7443",
        "--controller-ca",
        "/ca.pem",
    ];
    let cert = [
        "--controller-cert",
        "/runner.pem",
        "--controller-key",
        "/runner.key",
    ];
    let full = [
        &tls[..],
        &cert,
        &["--service-key", "k", "--advertise", "10.0.0.7:7701"],
    ]
    .concat();
    assert!(parse(&full).is_ok());
    assert!(parse(&tls).is_err(), "no key or address");
    for left_out in ["--controller-cert", "--controller-key"] {
        let at = full.iter().position(|a| *a == left_out).unwrap();
        let without = [&full[..at], &full[at + 2..]].concat();
        assert!(parse(&without).is_err(), "no {left_out}");
    }
    assert!(parse(&[&full[..], &["--socket", "/r.sock"]].concat()).is_err());
    assert!(parse(&[]).is_err(), "neither --socket nor --controller");
}

#[test]
fn the_controller_listener_needs_agent_links_and_the_key() {
    let base = ["--socket", "/r.sock", "--links", "0.0.0.0:7701"];
    let agents = ["--agents", "0.0.0.0:7702"];
    let key = ["--service-key", "k"];
    assert!(parse(&[&base[..], &agents, &key].concat()).is_ok());
    assert!(parse(&[&base[..], &agents].concat()).is_err(), "no key");
    assert!(parse(&[&base[..], &key].concat()).is_err(), "no agents");
}
