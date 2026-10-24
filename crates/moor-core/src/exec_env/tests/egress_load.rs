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

fn listens(addr: &str, port: u16) -> bool {
    std::fs::read_to_string("/proc/net/tcp")
        .unwrap()
        .contains(&format!(" {addr}:{port:04X} 00000000:0000 0A"))
}

fn tunnels_through(proxy: (Ipv4Addr, u16)) -> bool {
    let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let target_port = target.local_addr().unwrap().port();
    let mut s = TcpStream::connect(proxy).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "CONNECT 127.0.0.1:{target_port} HTTP/1.1\r\n\r\n").unwrap();
    let mut head = [0u8; 12];
    s.read_exact(&mut head).is_ok() && head.ends_with(b" 200")
}

struct Daemon(std::path::PathBuf);

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(self.0.join(".moor/egress.pid")) {
            let _ = Command::new("kill").args(["--", pid.trim()]).status();
        }
    }
}

#[test]
#[ignore = "needs a real tinyproxy; CI installs tinyproxy-bin and runs it with --ignored"]
fn tinyproxy_serves_on_the_bridge_address_while_docker0_exists_and_drops_it_after() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let bin = home.join("bin");
    moor_testkit::write_host_guards(&bin).unwrap();
    let real = std::env::var_os("MOOR_TINYPROXY").map_or_else(
        || {
            std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .map(|d| d.join("tinyproxy"))
                .find(|p| p.exists())
                .expect("tinyproxy on PATH or MOOR_TINYPROXY")
        },
        std::path::PathBuf::from,
    );
    std::os::unix::fs::symlink(real, bin.join("tinyproxy")).unwrap();
    let _daemon = Daemon(home.clone());
    let port = free_port();
    let run = || {
        let script = format!(
            "{}\necho ran\n",
            super::super::egress::lines(port, free_port(), &[]).join("\n")
        );
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let out = Command::new("sh")
            .args(["-c", &script])
            .env("HOME", &home)
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("ran"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let second = Ipv4Addr::new(127, 0, 0, 2);
    std::fs::write(home.join("docker0.route"), "127.0.0.0/8 src 127.0.0.2\n").unwrap();
    run();
    assert!(listens("0100007F", port) && listens("0200007F", port));
    assert!(!listens("00000000", port));
    assert!(tunnels_through((Ipv4Addr::LOCALHOST, port)));
    assert!(tunnels_through((second, port)));

    std::fs::remove_file(home.join("docker0.route")).unwrap();
    run();
    assert!(listens("0100007F", port));
    assert!(
        !listens("0200007F", port),
        "the bridge listener outlived docker0"
    );
    assert!(tunnels_through((Ipv4Addr::LOCALHOST, port)));
}
