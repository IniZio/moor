//! The relay against a real herdr-core core of its own, standing in for the Box. The stand-in
//! connect command's home holds a `herdr-core` that speaks only to that core, so the relay runs
//! the same Box command it runs over ssh.

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moor_core::seam::clock::Clock;
use moor_core::view::{AttachEnd, AttachView};

use super::{run, Link};
use crate::adapters::box_agent::relay_argv;
use crate::adapters::clock::SystemClock;

/// The clock the tests wait on.
const CLOCK: SystemClock = SystemClock::real();

/// Serializes the tests here, since `ptys` counts the whole process's pty descriptors.
static SERIAL: Mutex<()> = Mutex::new(());

/// A herdr-core core of its own running the agent's terminal, stopped on drop.
struct Agent {
    /// The core's home: its state and the `herdr-core` wrapper.
    dir: tempfile::TempDir,
    /// Held while the core lives.
    _serial: std::sync::MutexGuard<'static, ()>,
}

impl Agent {
    /// A core whose `moor-agent` terminal runs `cat`; none without herdr-core.
    fn start() -> Option<Self> {
        let home = std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/bin"));
        let real = std::env::split_paths(&std::env::var_os("PATH")?)
            .chain(home)
            .map(|d| d.join("herdr-core"))
            .find(|p| p.is_file())?;
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join(".local/bin")).expect("bin");
        std::fs::create_dir(dir.path().join("run")).expect("run");
        let d = dir.path().display();
        let script = format!(
            "#!/bin/sh\nunset HC_SOCKET HC_SESSION HC_TERMINAL_ID HC_SCOPE\n\
             export HOME={d} XDG_RUNTIME_DIR={d}/run XDG_STATE_HOME={d}/state \
             XDG_CONFIG_HOME={d}/config XDG_DATA_HOME={d}/data XDG_CACHE_HOME={d}/cache\n\
             exec {} \"$@\"\n",
            real.display()
        );
        // Written by a child: a writable fd in this process leaks into other tests' forks and
        // makes exec of the wrapper fail with ETXTBSY.
        let draft = dir.path().join("herdr-core.sh");
        std::fs::write(&draft, script).expect("wrapper");
        let installed = Command::new("install")
            .args(["-m", "755"])
            .arg(&draft)
            .arg(dir.path().join(".local/bin/herdr-core"))
            .status()
            .expect("install");
        assert!(installed.success(), "install the wrapper");
        let agent = Self {
            dir,
            _serial: SERIAL
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        };
        agent.hc(&["run", "--bg", "--no-parent", "--name", "moor-agent", "cat"]);
        Some(agent)
    }

    /// Runs herdr-core on the private core and returns its trimmed output.
    fn hc(&self, args: &[&str]) -> String {
        let out = Command::new(self.dir.path().join(".local/bin/herdr-core"))
            .args(args)
            .output()
            .expect("herdr-core");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// The stand-in for the Box's connect argv, whose home is the private one, where the Box
    /// command looks for herdr-core. Like ssh it runs its last argument as a shell line in a
    /// child, and exits 255 when that child dies by a signal, as a lost link does.
    fn connect(&self) -> Vec<String> {
        let line = format!(
            "HOME={}; export HOME; sh -c \"$1\"; c=$?; [ $c -gt 128 ] && exit 255; exit $c",
            self.dir.path().display()
        );
        ["sh", "-c", &line, "connect"].map(str::to_owned).to_vec()
    }

    /// The agent terminal's size, as `COLSxROWS`; empty once the terminal is gone.
    fn window(&self) -> String {
        let got: serde_json::Value =
            serde_json::from_str(&self.hc(&["get", "moor-agent", "--json"])).unwrap_or_default();
        match (got.get("cols"), got.get("rows")) {
            (Some(c), Some(r)) => format!("{c}x{r}"),
            _ => String::new(),
        }
    }

    /// The processes attached through this core.
    fn attached_pids(&self) -> Vec<i32> {
        let home = format!("HOME={}", self.dir.path().display());
        std::fs::read_dir("/proc")
            .expect("proc")
            .filter_map(|e| {
                let path = e.ok()?.path();
                let cmd = std::fs::read(path.join("cmdline")).ok()?;
                let env = std::fs::read(path.join("environ")).ok()?;
                let cmd = String::from_utf8_lossy(&cmd).replace('\0', " ");
                let env = String::from_utf8_lossy(&env);
                let mine = env.split('\0').any(|v| v == home);
                (mine && cmd.contains("attach moor-agent"))
                    .then(|| path.file_name()?.to_str()?.parse().ok())?
            })
            .collect()
    }

    /// How many processes are attached through this core.
    fn attached(&self) -> usize {
        self.attached_pids().len()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.hc(&["server", "stop"]);
    }
}

/// One relay over a socket pair, with the screen bytes the client has read.
struct Attached {
    /// The client's end.
    client: UnixStream,
    /// Every bytes frame payload the client got.
    screen: Arc<Mutex<Vec<u8>>>,
    /// Set to make the relay see a controller restart.
    stopping: Arc<AtomicBool>,
    /// The relay.
    relay: JoinHandle<std::io::Result<AttachView>>,
}

impl Attached {
    /// Starts the relay to `agent` at 120x33.
    fn open(agent: &Agent) -> Self {
        let (client, server) = UnixStream::pair().expect("pair");
        let argv = relay_argv(&agent.connect());
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let relay = thread::spawn(move || {
            let mut input = BufReader::new(server.try_clone().expect("clone"));
            let mut output = server.try_clone().expect("clone");
            let unblock = || {
                let _ = server.shutdown(Shutdown::Read);
            };
            let stopping = || stop.load(Ordering::SeqCst);
            let link = Link {
                id: "1",
                input: &mut input,
                output: &mut output,
                unblock: &unblock,
                stopping: &stopping,
            };
            run(&argv, 120, 33, link)
        });
        let screen = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&screen);
        let mut down = BufReader::new(client.try_clone().expect("clone"));
        thread::spawn(move || read_screen(&mut down, &seen));
        Self {
            client,
            screen,
            stopping,
            relay,
        }
    }

    /// The screen bytes read so far, as text.
    fn screen(&self) -> String {
        String::from_utf8_lossy(&self.screen.lock().expect("screen")).into_owned()
    }

    /// Sends one frame line and the raw bytes after it.
    fn send(&mut self, line: &serde_json::Value, raw: &[u8]) {
        let mut frame = line.to_string().into_bytes();
        frame.push(b'\n');
        frame.extend_from_slice(raw);
        self.client.write_all(&frame).expect("send");
    }

    /// Waits for the relay to end.
    fn ended(self) -> AttachView {
        let deadline = CLOCK.instant() + Duration::from_secs(10);
        while !self.relay.is_finished() {
            assert!(CLOCK.instant() < deadline, "the relay did not end");
            thread::sleep(Duration::from_millis(20));
        }
        self.relay.join().expect("relay").expect("relay ran")
    }
}

/// Collects the payload of every bytes frame from `down` into `screen`.
fn read_screen(down: &mut impl BufRead, screen: &Mutex<Vec<u8>>) {
    let mut line = String::new();
    while matches!(down.read_line(&mut line), Ok(n) if n > 0) {
        let frame: serde_json::Value = serde_json::from_str(line.trim_end()).expect("frame");
        line.clear();
        if frame.get("frame").and_then(serde_json::Value::as_str) == Some("bytes") {
            let len = frame
                .get("len")
                .and_then(serde_json::Value::as_u64)
                .expect("len");
            let len = usize::try_from(len).expect("len");
            let mut buf = vec![0u8; len];
            down.read_exact(&mut buf).expect("bytes");
            screen.lock().expect("screen").extend_from_slice(&buf);
        }
    }
}

/// Waits up to ten seconds for `cond`.
fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let deadline = CLOCK.instant() + Duration::from_secs(10);
    while !cond() {
        assert!(CLOCK.instant() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Pty descriptors this process holds.
fn ptys() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .expect("fds")
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter(|p: &PathBuf| p.starts_with(Path::new("/dev/pts")) || p == Path::new("/dev/ptmx"))
        .count()
}

/// The agent's core, or a skip note when the host has no herdr-core.
fn agent() -> Option<Agent> {
    let agent = Agent::start();
    if agent.is_none() {
        let _ = writeln!(std::io::stderr(), "skipped: herdr-core is not installed");
    }
    agent
}

#[test]
fn keys_reach_the_agent_and_its_screen_and_size_follow_the_client_until_it_shuts() {
    let Some(agent) = agent() else { return };
    let before = ptys();
    let mut relay = Attached::open(&agent);
    wait_for("the client's size", || agent.window() == "120x33");
    relay.send(
        &serde_json::json!({"frame": "bytes", "id": "1", "len": 11}),
        b"hello-moor\r",
    );
    wait_for("the typed line on the screen", || {
        relay.screen().contains("hello-moor")
    });
    relay.send(
        &serde_json::json!({"frame": "resize", "id": "1", "cols": 100, "rows": 30}),
        b"",
    );
    wait_for("the resized window", || agent.window() == "100x30");
    relay.client.shutdown(Shutdown::Write).expect("shut");
    let view = relay.ended();
    assert_eq!(view.ended_by, AttachEnd::Client);
    assert_eq!(view.bytes_in, 11);
    assert!(view.bytes_out > 0, "{view:?}");
    wait_for("no attached client", || agent.attached() == 0);
    assert_eq!(ptys(), before, "the relay's pty was closed");
}

#[test]
fn the_agent_window_returns_to_its_own_size_after_a_relay() {
    let Some(agent) = agent() else { return };
    let relay = Attached::open(&agent);
    wait_for("the client's size", || agent.window() == "120x33");
    relay.client.shutdown(Shutdown::Write).expect("shut");
    assert_eq!(relay.ended().ended_by, AttachEnd::Client);
    wait_for("the agent's own size back", || agent.window() == "200x120");
}

#[test]
fn the_agent_session_ending_ends_the_relay_as_the_agents() {
    let Some(agent) = agent() else { return };
    let relay = Attached::open(&agent);
    wait_for("the attach", || agent.attached() == 1);
    agent.hc(&["kill", "moor-agent"]);
    assert_eq!(relay.ended().ended_by, AttachEnd::Agent);
    assert_eq!(agent.attached(), 0);
}

#[test]
fn a_dropped_link_ends_the_relay_as_the_links() {
    let Some(agent) = agent() else { return };
    let relay = Attached::open(&agent);
    wait_for("the attach", || agent.attached() == 1);
    let pid = *agent.attached_pids().first().expect("pid");
    let pid = rustix::process::Pid::from_raw(pid).expect("pid");
    rustix::process::kill_process(pid, rustix::process::Signal::KILL).expect("kill");
    assert_eq!(relay.ended().ended_by, AttachEnd::Link);
    assert_eq!(agent.attached(), 0);
    assert!(!agent.window().is_empty(), "the agent outlives the link");
}

#[test]
fn a_controller_restart_ends_the_relay_as_the_links() {
    let Some(agent) = agent() else { return };
    let relay = Attached::open(&agent);
    wait_for("the attach", || agent.attached() == 1);
    relay.stopping.store(true, Ordering::SeqCst);
    assert_eq!(relay.ended().ended_by, AttachEnd::Link);
    wait_for("no attached client", || agent.attached() == 0);
}
