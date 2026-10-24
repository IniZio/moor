//! A fake egress proxy for tests that run the Box prelude under a real `sh`.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher as _, Hasher as _};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rustix::process::{kill_process, Pid, Signal};
use socket2::{Domain, Socket, Type};

use crate::write_executable;

/// A fake `tinyproxy` that records each start.
///
/// It logs its argv to `$HOME/fake.log`, runs a placeholder process named `tinyproxy` in a session
/// of its own, as the real daemon detaches, writes that pid to the config's `PidFile`, and records
/// the start in `$HOME/fake-starts`.
pub const FAKE_TINYPROXY: &str = r#"#!/bin/sh
echo "tinyproxy $*" >>"$HOME/fake.log"
pid=$(sed -n 's/^PidFile "\(.*\)"$/\1/p' "$2")
daemon="$HOME/.fake-tinyproxy/tinyproxy"
[ -x "$daemon" ] || { mkdir -p "$HOME/.fake-tinyproxy" && cp "$(command -v sleep)" "$daemon"; }
rm -f "$pid"
setsid sh -c 'echo $$ >"$1.tmp" && mv -f "$1.tmp" "$1" && exec "$2" 30' sh "$pid" "$daemon" </dev/null >/dev/null 2>&1 &
n=0
while [ ! -s "$pid" ]; do
  n=$((n + 1))
  [ "$n" -le 500 ] || exit 1
  sleep 0.01
done
echo start >>"$HOME/fake-starts"
"#;

/// Stands in for `sudo` and `apt-get` so a Box script that a test runs on this host never runs
/// the host's own: each names itself on stderr and fails with exit 97.
pub const HOST_GUARD: &str = "#!/bin/sh\n\
    echo \"a test ran $(basename \"$0\") $*; the host's own is never run\" >&2\n\
    exit 97\n";

/// Writes [`HOST_GUARD`] as `sudo` and `apt-get` into `bin`.
///
/// A test that runs a Box script with the host's `PATH` after its own `bin` calls this, so the
/// script never reaches the host's tools. A fake the test already wrote under either name is kept.
pub fn write_host_guards(bin: &Path) -> io::Result<()> {
    std::fs::create_dir_all(bin)?;
    for name in ["sudo", "apt-get"] {
        let path = bin.join(name);
        if !path.exists() {
            write_executable(&path, HOST_GUARD)?;
        }
    }
    Ok(())
}

/// The fake `tinyproxy` in a `bin` directory and the loopback port it "listens" on: the port opens
/// each time the fake records a start, and closes on [`FakeEgress::close`].
///
/// A socket that never listens holds the port for the fake's whole life. The prelude reads the
/// port as free until a start opens it, and no other socket on the host gets the port: the tests
/// share the host's ports with every other test process.
pub struct FakeEgress {
    /// The `HOME` the prelude runs with.
    home: PathBuf,
    /// The port the egress proxy listens on.
    port: u16,
    /// Holds `port` against other binds and outgoing connections for as long as this lives.
    _held: Socket,
    /// Claims `port` among the fakes that [`FakeEgress::start_in`] places, for as long as this lives.
    #[allow(dead_code, reason = "held for its lifetime, never read")]
    claim: Option<Socket>,
    /// The listener while the port is open.
    open: Arc<Mutex<Option<TcpListener>>>,
    /// Tells the watcher to stop.
    done: Arc<AtomicBool>,
    /// The thread that opens the port on each start.
    watcher: Option<JoinHandle<()>>,
}

impl FakeEgress {
    /// Writes the fake `tinyproxy` into `bin` and watches `home` for its starts, on a free port.
    pub fn start(home: &Path, bin: &Path) -> io::Result<Self> {
        Self::start_on(home, bin, 0)
    }

    /// [`Self::start`] on `port`, for a test whose egress port moor derives.
    pub fn start_on(home: &Path, bin: &Path, port: u16) -> io::Result<Self> {
        let held = Socket::new(Domain::IPV4, Type::STREAM, None)?;
        held.set_reuse_address(true)?;
        held.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)).into())?;
        let port = held
            .local_addr()?
            .as_socket()
            .map_or(port, |addr| addr.port());
        write_executable(&bin.join("tinyproxy"), FAKE_TINYPROXY)?;
        let open: Arc<Mutex<Option<TcpListener>>> = Arc::new(Mutex::new(None));
        let done = Arc::new(AtomicBool::new(false));
        let starts = home.join("fake-starts");
        let (o, d) = (Arc::clone(&open), Arc::clone(&done));
        let watcher = std::thread::spawn(move || {
            let mut seen = 0;
            while !d.load(Ordering::Relaxed) {
                let n = std::fs::read_to_string(&starts).map_or(0, |s| s.lines().count());
                if n > seen {
                    seen = n;
                    if let Ok(mut slot) = o.lock() {
                        *slot = TcpListener::bind(("127.0.0.1", port)).ok();
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(Self {
            home: home.to_path_buf(),
            port,
            _held: held,
            claim: None,
            open,
            done,
            watcher: Some(watcher),
        })
    }

    /// [`Self::start_on`] on a port of `ports` that no other fake on the host holds.
    ///
    /// Each fake claims its port on `127.0.0.2`, without `SO_REUSEADDR`, so two test processes that
    /// start together never take the same one.
    pub fn start_in(home: &Path, bin: &Path, ports: RangeInclusive<u16>) -> io::Result<Self> {
        let (first, last) = (u32::from(*ports.start()), u32::from(*ports.end()));
        let span = last - first + 1;
        let from = RandomState::new().build_hasher().finish() % u64::from(span);
        for step in 0..span {
            let port = u16::try_from(first + (u32::try_from(from).unwrap_or(0) + step) % span)
                .unwrap_or_else(|_| *ports.start());
            let claim = Socket::new(Domain::IPV4, Type::STREAM, None)?;
            if claim
                .bind(&SocketAddr::from((Ipv4Addr::new(127, 0, 0, 2), port)).into())
                .is_err()
            {
                continue;
            }
            if let Ok(mut fake) = Self::start_on(home, bin, port) {
                fake.claim = Some(claim);
                return Ok(fake);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "no free port in the range",
        ))
    }

    /// The loopback port the prelude must name as the egress port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Opens the port with no start recorded, as another process in the Box that holds it.
    pub fn occupy(&self) {
        if let Ok(mut slot) = self.open.lock() {
            *slot = TcpListener::bind(("127.0.0.1", self.port)).ok();
        }
    }

    /// [`Self::occupy`] on every IPv4 address, as a process that listens on a wildcard.
    pub fn occupy_wildcard(&self) {
        if let Ok(mut slot) = self.open.lock() {
            *slot = TcpListener::bind(("0.0.0.0", self.port)).ok();
        }
    }

    /// Closes the port, as a tinyproxy that died leaves it.
    pub fn close(&self) {
        if let Ok(mut slot) = self.open.lock() {
            *slot = None;
        }
    }
}

impl Drop for FakeEgress {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(w) = self.watcher.take() {
            let _ = w.join();
        }
        if let Some(pid) = self.daemon() {
            let _ = kill_process(pid, Signal::KILL);
            for _ in 0..200 {
                if !running(pid) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

impl FakeEgress {
    /// The fake daemon the last start left running, by the pid it wrote.
    fn daemon(&self) -> Option<Pid> {
        let text = std::fs::read_to_string(self.home.join(".moor/egress.pid")).ok()?;
        let pid = Pid::from_raw(text.trim().parse().ok()?)?;
        let comm = std::fs::read_to_string(format!("/proc/{}/comm", pid.as_raw_nonzero())).ok()?;
        (comm.trim() == "tinyproxy" && running(pid)).then_some(pid)
    }
}

/// Whether `pid` exists and has not exited; a zombie its parent has not reaped counts as gone.
fn running(pid: Pid) -> bool {
    std::fs::read_to_string(format!("/proc/{}/stat", pid.as_raw_nonzero())).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z" && state != "X")
    })
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use socket2::{Domain, Socket, Type};

    use super::{running, FakeEgress, Pid};

    /// Binds a socket without `SO_REUSEADDR` to `port`, as another process on the host would.
    fn bind_elsewhere(port: u16) -> std::io::Result<Socket> {
        let other = Socket::new(Domain::IPV4, Type::STREAM, None)?;
        other.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)).into())?;
        Ok(other)
    }

    #[test]
    fn two_fakes_started_in_one_range_hold_different_ports() {
        let dir = tempfile::tempdir().unwrap();
        let a = FakeEgress::start_in(dir.path(), dir.path(), 41000..=41001).unwrap();
        let b = FakeEgress::start_in(dir.path(), dir.path(), 41000..=41001).unwrap();
        assert_ne!(a.port(), b.port());
        assert!(FakeEgress::start_in(dir.path(), dir.path(), 41000..=41001).is_err());
    }

    #[test]
    fn no_other_socket_takes_the_port_while_the_fake_lives() {
        let dir = tempfile::tempdir().unwrap();
        let egress = FakeEgress::start(dir.path(), dir.path()).unwrap();
        let port = egress.port();
        assert!(
            bind_elsewhere(port).is_err(),
            "another socket bound port {port}"
        );
    }

    #[test]
    fn the_daemon_leaves_the_starting_group_and_dies_with_the_fake() {
        let dir = tempfile::tempdir().unwrap();
        let (home, bin) = (dir.path().join("home"), dir.path().join("bin"));
        std::fs::create_dir_all(home.join(".moor")).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let egress = FakeEgress::start(&home, &bin).unwrap();
        let conf = dir.path().join("tinyproxy.conf");
        let pid_file = home.join(".moor/egress.pid");
        std::fs::write(&conf, format!("PidFile \"{}\"\n", pid_file.display())).unwrap();
        let status = std::process::Command::new(bin.join("tinyproxy"))
            .arg("-c")
            .arg(&conf)
            .env("HOME", &home)
            .status()
            .unwrap();
        assert!(status.success());
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let pid = Pid::from_raw(pid).unwrap();
        let group = rustix::process::getpgid(Some(pid)).unwrap();
        assert_eq!(group, pid, "the daemon leads a group of its own");
        drop(egress);
        assert!(!running(pid), "the daemon died with the fake");
    }
}
