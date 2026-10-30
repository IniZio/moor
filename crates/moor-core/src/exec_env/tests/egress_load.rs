//! A real `tinyproxy`, started by moor's egress lines as on a Box, under the parallel load a build
//! tool opens.

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::super::egress::lines;

/// Tunnels held open at once: Gradle and the Android plugin open well over a hundred while they
/// resolve dependencies.
const TUNNELS: usize = 200;

/// The pid file of a tinyproxy the egress lines started, killed on drop.
struct Tinyproxy(PathBuf);

impl Drop for Tinyproxy {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(&self.0) {
            let _ = Command::new("kill").arg(pid.trim()).status();
        }
    }
}

fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Runs the egress lines with `home` as the Box home, which return once tinyproxy listens.
fn start(home: &Path) -> (Tinyproxy, u16) {
    let port = free_port();
    let out = Command::new("sh")
        .arg("-c")
        .arg(lines(port, free_port(), &[]).join("\n"))
        .env("HOME", home)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let proxy = Tinyproxy(home.join(".moor/egress.pid"));
    assert!(
        out.status.success(),
        "tinyproxy never listened on {port}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
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
