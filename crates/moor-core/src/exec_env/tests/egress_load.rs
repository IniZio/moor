//! moor's egress config run by a real `tinyproxy` under the parallel load a build tool opens.

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::super::egress::config;

/// Tunnels held open at once: Gradle and the Android plugin open well over a hundred while they
/// resolve dependencies.
const TUNNELS: usize = 200;

/// A `tinyproxy -d` child, killed on drop.
struct Tinyproxy(Child);

impl Drop for Tinyproxy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Starts tinyproxy from moor's config on a free loopback port and waits until it listens.
fn start(dir: &std::path::Path) -> (Tinyproxy, u16) {
    let port = free_port();
    let conf = dir.join("egress.conf");
    let pid = dir.join("egress.pid");
    let text = format!(
        "{}PidFile \"{}\"\n",
        config(port, free_port(), &[]),
        pid.display()
    );
    std::fs::write(&conf, text).unwrap();
    let child =
        Command::new(std::env::var_os("MOOR_TINYPROXY").unwrap_or_else(|| "tinyproxy".into()))
            .arg("-d")
            .arg("-c")
            .arg(&conf)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("tinyproxy on PATH or MOOR_TINYPROXY");
    let proxy = Tinyproxy(child);
    let mut polls = 0;
    while TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err() {
        polls += 1;
        assert!(polls < 250, "tinyproxy never listened on {port}");
        std::thread::sleep(Duration::from_millis(20));
    }
    (proxy, port)
}

#[test]
#[ignore = "needs a real tinyproxy; CI installs tinyproxy-bin and runs it with --ignored"]
fn every_one_of_many_parallel_tunnels_is_answered_while_all_stay_open() {
    let dir = tempfile::tempdir().unwrap();
    let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let target_port = target.local_addr().unwrap().port();
    std::thread::spawn(move || target.incoming().collect::<Vec<_>>());
    let (_proxy, port) = start(dir.path());

    let mut tunnels: Vec<TcpStream> = (0..TUNNELS)
        .map(|_| {
            let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            write!(
                s,
                "CONNECT 127.0.0.1:{target_port} HTTP/1.1\r\nHost: 127.0.0.1:{target_port}\r\n\r\n"
            )
            .unwrap();
            s
        })
        .collect();

    // Once one tunnel waited 5 s, every later one has waited at least as long.
    let mut unanswered = Vec::new();
    for (i, s) in tunnels.iter_mut().enumerate() {
        let wait = if unanswered.is_empty() { 5000 } else { 1 };
        s.set_read_timeout(Some(Duration::from_millis(wait)))
            .unwrap();
        let mut head = [0u8; 12];
        let answered = s.read_exact(&mut head).is_ok() && head.ends_with(b" 200");
        if !answered {
            unanswered.push(i);
        }
    }
    assert_eq!(
        unanswered,
        Vec::<usize>::new(),
        "tunnels with no 200 within 5 s while {TUNNELS} were open"
    );
}
