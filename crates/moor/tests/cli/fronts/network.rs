//! A front has its own network with nothing but its loopback: no loopback listener of the
//! host or of another front is reachable from it, and it has no route out.

use std::fs;
use std::io::{BufRead as _, BufReader};
use std::net::{Ipv6Addr, TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::process::{Child, ChildStdout, Command, Stdio};

use rustix::process::{kill_process, Pid, Signal};
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

/// Prints the front Proxy port pid 1 bridged and whether the front reaches it, then waits.
const BRIDGED: &str = r#"
import os, socket, time
port = os.environ["MOOR_FRONT_PROXY_PORT"]
s = socket.socket()
s.settimeout(2)
s.connect(("127.0.0.1", int(port)))
print(port, "connected", flush=True)
time.sleep(600)
"#;

/// Starts `moor front-shim` as a child of this test, with `flags` and `command`, its stdout
/// piped; returns it and its stdout lines.
fn spawn_shim(flags: &[&str], command: &[&str]) -> (Child, std::io::Lines<BufReader<ChildStdout>>) {
    let me = std::process::id().to_string();
    let mut child = Command::new(env!("CARGO_BIN_EXE_moor"))
        .args(["front-shim", "--parent", &me])
        .args(flags)
        .arg("--")
        .args(command)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let lines = BufReader::new(child.stdout.take().unwrap()).lines();
    (child, lines)
}

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

/// Kills `child` and reaps it.
fn kill(mut child: Child) {
    let pid = Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
    let _ = kill_process(pid, Signal::KILL);
    let _ = child.wait();
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

#[test]
fn a_front_cannot_reach_another_fronts_proxy_bridge() {
    let dir = fresh_dir_outside_tmp("bridges");
    let socket = dir.join("proxy.sock");
    let _proxy = UnixListener::bind(&socket).unwrap();
    let socket = socket.to_str().unwrap();
    let (first, mut said) = spawn_shim(&["--proxy", socket], &["python3", "-c", BRIDGED]);
    let line = said.next().unwrap().unwrap();
    let (port, reached) = line.split_once(' ').unwrap();
    assert_eq!(reached, "connected", "the front reaches its own bridge");
    let got = reach(&[format!("127.0.0.1:{port}")]);
    kill(first);
    assert_eq!(
        got[format!("127.0.0.1:{port}")],
        "ConnectionRefusedError",
        "{got}"
    );
}
