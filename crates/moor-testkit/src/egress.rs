//! A fake egress proxy for tests that run the Box prelude under a real `sh`.

use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::write_executable;

/// A fake `tinyproxy`: logs its argv to `$HOME/fake.log`, runs a placeholder process whose pid it
/// writes to the config's `PidFile`, and records the start in `$HOME/fake-starts`.
pub const FAKE_TINYPROXY: &str = r#"#!/bin/sh
echo "tinyproxy $*" >>"$HOME/fake.log"
pid=$(sed -n 's/^PidFile "\(.*\)"$/\1/p' "$2")
sleep 30 >/dev/null 2>&1 &
echo $! >"$pid"
echo start >>"$HOME/fake-starts"
"#;

/// Stands in for `sudo` and `apt-get` so a Box script that a test runs on this host never runs
/// the host's own: each names itself on stderr and fails with exit 97.
pub const HOST_GUARD: &str = "#!/bin/sh\n\
    echo \"a test ran $(basename \"$0\") $*; the host's own is never run\" >&2\n\
    exit 97\n";

/// Writes [`HOST_GUARD`] as `sudo` and `apt-get` into `bin`, which a fake ssh puts first on the
/// `PATH` it exports to the Box command.
pub fn write_host_guards(bin: &Path) -> io::Result<()> {
    std::fs::create_dir_all(bin)?;
    for name in ["sudo", "apt-get"] {
        write_executable(&bin.join(name), HOST_GUARD)?;
    }
    Ok(())
}

/// The fake `tinyproxy` in a `bin` directory and the loopback port it "listens" on: the port opens
/// each time the fake records a start, and closes on [`FakeEgress::close`].
pub struct FakeEgress {
    /// The `HOME` the prelude runs with.
    home: PathBuf,
    /// The port the egress proxy listens on.
    port: u16,
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
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        Self::start_on(home, bin, port)
    }

    /// [`Self::start`] on `port`, for a test whose egress port moor derives.
    pub fn start_on(home: &Path, bin: &Path, port: u16) -> io::Result<Self> {
        write_host_guards(bin)?;
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
