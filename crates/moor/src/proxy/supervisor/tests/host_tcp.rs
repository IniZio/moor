//! The front listens only on its Unix socket: no TCP port of the host answers as the front.

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::time::Duration;

use super::front_e2e::REFUSAL;
use super::{start, World};
use crate::testing::bridge::bridge;

/// How long a probe waits to connect and then for a reply.
const PROBE: Duration = Duration::from_millis(500);

/// The full name of [`no_tcp_listener_of_this_process_answers_as_the_front`].
const TCP_CHILD: &str =
    "proxy::supervisor::tests::host_tcp::no_tcp_listener_of_this_process_answers_as_the_front";

/// The local address of every TCP socket this process listens on.
fn tcp_listeners() -> Vec<SocketAddr> {
    let ours: HashSet<String> = fs::read_dir("/proc/self/fd")
        .unwrap()
        .flatten()
        .filter_map(|fd| fs::read_link(fd.path()).ok())
        .filter_map(|t| {
            Some(
                t.to_str()?
                    .strip_prefix("socket:[")?
                    .strip_suffix(']')?
                    .to_owned(),
            )
        })
        .collect();
    let tables =
        ["/proc/net/tcp", "/proc/net/tcp6"].map(|t| fs::read_to_string(t).unwrap_or_default());
    tables
        .iter()
        .flat_map(|table| table.lines().skip(1))
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            let listening = cols.get(3) == Some(&"0A") && ours.contains(*cols.get(9)?);
            listening.then(|| local_addr(cols.get(1)?)).flatten()
        })
        .collect()
}

/// Parses a procfs local address such as `0100007F:1F90`; a wildcard becomes loopback.
fn local_addr(column: &str) -> Option<SocketAddr> {
    let (hex, port) = column.split_once(':')?;
    let mut bytes = Vec::new();
    for word in hex.as_bytes().chunks(8) {
        let word = u32::from_str_radix(std::str::from_utf8(word).ok()?, 16).ok()?;
        bytes.extend(word.to_ne_bytes());
    }
    let ip = match <[u8; 4]>::try_from(bytes.as_slice()) {
        Ok(v4) => IpAddr::from(v4),
        Err(_) => IpAddr::from(<[u8; 16]>::try_from(bytes.as_slice()).ok()?),
    };
    let ip = match ip {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    Some(SocketAddr::new(ip, u16::from_str_radix(port, 16).ok()?))
}

/// The TCP listeners of this process that refuse an origin-form request the way the front does.
fn fronts_on_tcp() -> Vec<SocketAddr> {
    let answers_as_front = |addr: &SocketAddr| {
        let Ok(mut tcp) = TcpStream::connect_timeout(addr, PROBE) else {
            return false;
        };
        let _ = tcp.set_read_timeout(Some(PROBE));
        let _ = tcp.write_all(b"GET /x HTTP/1.1\r\nHost: x\r\n\r\n");
        let mut reply = Vec::new();
        let _ = tcp.read_to_end(&mut reply);
        String::from_utf8_lossy(&reply).contains(REFUSAL)
    };
    tcp_listeners()
        .into_iter()
        .filter(answers_as_front)
        .collect()
}

/// Body of the child process run by [`the_front_listens_on_no_host_tcp_port`]. It probes every
/// TCP listener of its process, so it must not share the process with other tests.
#[test]
#[ignore = "runs only as the child of another test"]
fn no_tcp_listener_of_this_process_answers_as_the_front() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    assert_eq!(fronts_on_tcp(), Vec::<SocketAddr>::new());

    let relay = bridge(run.socket());
    let relay = SocketAddr::from((Ipv4Addr::LOCALHOST, relay));
    assert_eq!(fronts_on_tcp(), vec![relay]);
}

#[test]
fn the_front_listens_on_no_host_tcp_port() {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "--test-threads=1", TCP_CHILD])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "{stdout}{stderr}"
    );
}
