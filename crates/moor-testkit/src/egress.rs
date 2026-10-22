//! A fake egress proxy for tests that run the Box prelude under a real `sh`.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use socket2::{Domain, Socket, Type};

use crate::write_executable;

/// A fake `tinyproxy` that records each start.
///
/// It logs its argv to `$HOME/fake.log`, runs a placeholder process named `tinyproxy`, as the real
/// daemon is, writes that pid to the config's `PidFile`, and records the start in
/// `$HOME/fake-starts`.
pub const FAKE_TINYPROXY: &str = r#"#!/bin/sh
echo "tinyproxy $*" >>"$HOME/fake.log"
pid=$(sed -n 's/^PidFile "\(.*\)"$/\1/p' "$2")
daemon="$HOME/.fake-tinyproxy/tinyproxy"
[ -x "$daemon" ] || { mkdir -p "$HOME/.fake-tinyproxy" && cp "$(command -v sleep)" "$daemon"; }
"$daemon" 30 >/dev/null 2>&1 &
echo $! >"$pid"
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
            open,
            done,
            watcher: Some(watcher),
        })
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
        if let Ok(pid) = std::fs::read_to_string(self.home.join(".moor/egress.pid")) {
            let _ = std::process::Command::new("kill").arg(pid.trim()).status();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use socket2::{Domain, Socket, Type};

    use super::FakeEgress;

    /// Binds a socket without `SO_REUSEADDR` to `port`, as another process on the host would.
    fn bind_elsewhere(port: u16) -> std::io::Result<Socket> {
        let other = Socket::new(Domain::IPV4, Type::STREAM, None)?;
        other.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)).into())?;
        Ok(other)
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
}
