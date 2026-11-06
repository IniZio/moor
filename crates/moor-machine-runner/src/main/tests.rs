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
fn a_tls_controller_takes_the_place_of_the_socket_and_needs_its_ca_key_and_address() {
    let tls = [
        "--controller",
        "tls://ctl:7443",
        "--controller-ca",
        "/ca.pem",
    ];
    let full = [
        &tls[..],
        &["--service-key", "k", "--advertise", "10.0.0.7:7701"],
    ]
    .concat();
    assert!(parse(&full).is_ok());
    assert!(parse(&tls).is_err(), "no key or address");
    assert!(parse(&[&full[..], &["--socket", "/r.sock"]].concat()).is_err());
    assert!(parse(&[]).is_err(), "neither --socket nor --controller");
}
