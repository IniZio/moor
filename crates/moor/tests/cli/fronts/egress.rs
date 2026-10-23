//! A front reaches the public internet only through its egress proxy, which refuses every
//! private, CGNAT, link-local, loopback and host address, any port but 443 and 80, and a name
//! whose DNS answer is private, and which a second DNS answer cannot steer. Without the proxy
//! the front has no route at all. A listed port reaches it from the host's `127.0.0.1` alone.

use std::process::{Command, Stdio};

use serde_json::{json, Value};

/// Stands in for a host: dummy interfaces with the addresses of a docker0, a tailscale0, an
/// lxdbr0, a ULA bridge and a public address of its own, a listener on port 443 of all of
/// them, a remote host behind a veth listening on 443, 80 and 8443 at a public address and at
/// a LAN and a CGNAT address that are not the host's, and a resolver on
/// 127.0.0.53 that answers `private.*` with the docker0 address, `rebind.*` first with the
/// remote and then with the docker0 address, and every other name with the remote. Runs a
/// front that asks its proxy for each target in turn and connects to the remote directly,
/// and prints what each listener says to the host, what the front got, the front's log and
/// the answers `rebind.*` got.
const HOST: &str = include_str!("egress_host.py");

/// Runs `script` with the moor binary and `args` as root of new user, network, PID and mount
/// namespaces, standing in for a host; returns the JSON it prints.
fn as_root_of_a_host(script: &str, args: &[&str]) -> Value {
    let out = Command::new("unshare")
        .args([
            "--user",
            "--map-root-user",
            "--net",
            "--pid",
            "--fork",
            "--mount",
            "--mount-proc",
        ])
        .args(["--", "python3", "-c", script, env!("CARGO_BIN_EXE_moor")])
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&said)
        .unwrap_or_else(|_| panic!("{said} {}", String::from_utf8_lossy(&out.stderr)))
}

/// What the front got in the stand-in host, and the stand-in's controls.
fn front_in_a_host() -> (Value, Value) {
    let got = as_root_of_a_host(HOST, &[]);
    let front: Value =
        serde_json::from_str(got["front"].as_str().unwrap()).unwrap_or_else(|_| panic!("{got}"));
    (got, front)
}

#[test]
fn a_front_reaches_the_public_internet_through_its_proxy_and_nothing_private() {
    let (got, front) = front_in_a_host();
    let controls = json!({
        "172.17.0.1": "host", "100.64.0.156": "host", "10.235.225.1": "host", "fc00::1": "host",
        "203.0.113.5": "host", "198.51.100.7": "remote", "192.168.77.7": "remote",
        "100.64.9.9": "remote",
    });
    assert_eq!(
        got["control"], controls,
        "the host reaches each listener: {got}"
    );
    for target in [
        "172.17.0.1:443",
        "192.168.77.7:443",
        "100.64.9.9:443",
        "100.64.0.156:443",
        "10.235.225.1:443",
        "[fc00::1]:443",
        "203.0.113.5:443",
        "127.0.0.1:443",
        "[::1]:443",
        "localhost:443",
        "198.51.100.7:8443",
        "private.test:443",
    ] {
        assert_eq!(front[target], json!([403, ""]), "{target}: {got}");
    }
    for target in ["198.51.100.7:443", "198.51.100.7:80", "remote.test:443"] {
        assert_eq!(front[target], json!([200, "remote"]), "{target}: {got}");
    }
    assert_eq!(front["direct"], "Network is unreachable", "{got}");
    assert_eq!(
        front["caps"],
        json!(["0000000000000000", "0000000000000000", "0000000000000000"]),
        "no effective, bounding or ambient capability: {got}"
    );
}

#[test]
fn a_second_dns_answer_cannot_steer_the_proxy_and_each_decision_is_logged() {
    let (got, front) = front_in_a_host();
    assert_eq!(front["rebind.test:443"], json!([200, "remote"]), "{got}");
    let log = got["log"].as_str().unwrap();
    for line in [
        "egress: host=rebind.test port=443 ip=198.51.100.7 decision=allow reason=public",
        "egress: host=private.test port=443 ip=172.17.0.1 decision=deny reason=private-range",
        "egress: host=203.0.113.5 port=443 ip=203.0.113.5 decision=deny reason=host-address",
        "egress: host=198.51.100.7 port=8443 ip=- decision=deny reason=port-not-443-or-80",
    ] {
        assert!(log.contains(line), "{line}: {log}");
    }
}

/// Gives a network namespace an outbound interface and a docker0-style address with a
/// listener, runs a front with one listed and one unlisted port, and prints what the front
/// reached of docker0 and what the namespace reached of the front at each of its addresses.
const PORTS: &str = include_str!("ports_host.py");

#[test]
fn a_listed_port_reaches_the_front_from_the_hosts_loopback_only() {
    let got = as_root_of_a_host(PORTS, &["18471", "18472"]);
    assert_eq!(got["127.0.0.1:listed"], "front", "{got}");
    for missed in ["10.9.0.2:listed", "172.17.0.1:listed", "127.0.0.1:unlisted"] {
        assert_eq!(got[missed], "ConnectionRefusedError", "{missed}: {got}");
    }
    assert_eq!(got["front_to_docker0"], "OSError", "no route out: {got}");
}
