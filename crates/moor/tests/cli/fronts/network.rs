//! A front has its own network with nothing but its loopback: no loopback listener of the
//! host or of another front is reachable from it, and it has no route out.

use std::fs;
use std::net::{Ipv6Addr, TcpListener, TcpStream};

use serde_json::Value;

use super::{fresh_dir_outside_tmp, shim};

/// Connects to each `host:port` of argv and prints what each got, its own network namespace,
/// its routes and its interfaces.
const REACH: &str = r#"
import json, os, socket, sys
def reach(host, port):
    s = socket.socket(socket.AF_INET6 if ":" in host else socket.AF_INET)
    s.settimeout(2)
    try:
        s.connect((host, int(port)))
        return "connected"
    except OSError as e:
        return type(e).__name__
targets = [a.rsplit(":", 1) for a in sys.argv[1:]]
got = {f"{h}:{p}": reach(h, p) for h, p in targets}
got["routes"] = open("/proc/net/route").readlines()[1:]
got["interfaces"] = sorted(l.split(":")[0].strip() for l in open("/proc/net/dev").readlines()[2:])
got["netns"] = os.readlink("/proc/self/ns/net")
print(json.dumps(got), flush=True)
"#;

/// What [`REACH`] got from inside a front for `targets`.
fn reach(targets: &[String]) -> Value {
    let dir = fresh_dir_outside_tmp("reach");
    let me = std::process::id().to_string();
    let mut args = vec!["--parent", &me, "--", "python3", "-c", REACH];
    args.extend(targets.iter().map(String::as_str));
    let out = shim(&dir, &args);
    let said = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&said)
        .unwrap_or_else(|_| panic!("{said} {}", String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn a_front_reaches_no_loopback_listener_of_the_host() {
    let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
    let v6 = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).ok();
    let mut targets = vec![format!("127.0.0.1:{}", v4.local_addr().unwrap().port())];
    targets.extend(
        v6.iter()
            .map(|l| format!("::1:{}", l.local_addr().unwrap().port())),
    );
    for target in &targets {
        let (host, port) = target.rsplit_once(':').unwrap();
        TcpStream::connect((host, port.parse::<u16>().unwrap()))
            .unwrap_or_else(|e| panic!("the host reaches its own {target}: {e}"));
    }
    let got = reach(&targets);
    for target in &targets {
        assert_eq!(got[target], "ConnectionRefusedError", "{target}: {got}");
    }
    assert_eq!(got["routes"], serde_json::json!([]), "no route out: {got}");
    assert_eq!(got["interfaces"], serde_json::json!(["lo"]), "{got}");
    let host_netns = fs::read_link("/proc/self/ns/net").unwrap();
    assert_ne!(got["netns"], host_netns.to_str().unwrap(), "{got}");
}
