//! Real moor processes a test starts, kills at chosen points and always cleans up.
//!
//! Each process the test starts leads a session of its own, and everything it starts stays in
//! that session, detached daemons included: they leave its process group, not its session. So
//! the sessions name every process the test caused, and teardown kills them all, wherever they
//! were reparented.

use std::io;
use std::process::Command;
use std::time::Duration;

use rustix::process::{kill_process, Pid, Signal};

use crate::ChildGuard;

/// How long teardown keeps killing before it gives up on a process that will not go.
const TEARDOWN: Duration = Duration::from_secs(10);

/// Pause between two looks at the fleet.
const STEP: Duration = Duration::from_millis(10);

/// How long a new process may take to become a session leader.
const SETSID: Duration = Duration::from_secs(5);

/// A process in one of the fleet's sessions.
#[derive(Debug, Clone)]
pub struct Member {
    /// Its pid.
    pub pid: Pid,
    /// Its argv, joined by spaces.
    pub cmdline: String,
}

/// The sessions a test started; dropping it kills every live process in them.
#[derive(Debug, Default)]
pub struct Fleet {
    /// Session leaders the test spawned, reaped by their guards.
    leaders: Vec<ChildGuard>,
    /// The session ids, which outlive their leaders.
    sessions: Vec<Pid>,
}

impl Fleet {
    /// An empty fleet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A command that runs `program` through `setsid(1)` with an empty environment, for
    /// [`Self::spawn`]. `setsid` runs the program in place, so its pid is the program's own.
    #[must_use]
    pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new("setsid");
        cmd.arg(program).env_clear();
        cmd
    }

    /// Starts `cmd`, made by [`Self::command`], as the leader of a new session; returns its pid,
    /// which is the session id.
    pub fn spawn(&mut self, cmd: &mut Command) -> io::Result<Pid> {
        let child = ChildGuard::new(cmd.spawn()?);
        let pid = Pid::from_child(&child);
        self.leaders.push(child);
        // Until `setsid` runs, the pid is still in the test's own session, which must never be
        // added to the fleet.
        let leads = || stat(pid).map(|s| s.session) == Some(pid.as_raw_nonzero().get());
        if !until(SETSID, leads) {
            return Err(io::Error::other(format!(
                "pid {pid:?} did not lead a session"
            )));
        }
        self.sessions.push(pid);
        Ok(pid)
    }

    /// Every live process in the fleet's sessions.
    #[must_use]
    pub fn members(&self) -> Vec<Member> {
        let sessions: Vec<i32> = self
            .sessions
            .iter()
            .map(|p| p.as_raw_nonzero().get())
            .collect();
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
            .filter_map(Pid::from_raw)
            .filter(|&pid| {
                stat(pid).is_some_and(|s| s.state != 'Z' && sessions.contains(&s.session))
            })
            .map(|pid| Member {
                pid,
                cmdline: cmdline(pid),
            })
            .collect()
    }

    /// The live member whose argv contains `needle`.
    #[must_use]
    pub fn find(&self, needle: &str) -> Option<Member> {
        self.members()
            .into_iter()
            .find(|m| m.cmdline.contains(needle))
    }

    /// Sends `SIGKILL` to `pid` and waits until it is gone: reaped when the test spawned it,
    /// else no longer a live member.
    pub fn kill(&mut self, pid: Pid) -> io::Result<()> {
        kill_process(pid, Signal::KILL)?;
        if let Some(i) = self.leaders.iter().position(|c| Pid::from_child(c) == pid) {
            self.leaders.remove(i).wait()?;
            return Ok(());
        }
        if until(TEARDOWN, || self.members().iter().all(|m| m.pid != pid)) {
            Ok(())
        } else {
            Err(io::Error::other(format!("pid {pid:?} outlived SIGKILL")))
        }
    }
}

impl Drop for Fleet {
    fn drop(&mut self) {
        until(TEARDOWN, || {
            let live = self.members();
            for m in &live {
                let _ = kill_process(m.pid, Signal::KILL);
            }
            for leader in &mut self.leaders {
                let _ = leader.try_wait();
            }
            live.is_empty()
        });
        self.leaders.clear();
    }
}

/// Checks `done` every [`STEP`] until it holds or `limit` has passed; whether it held.
fn until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let tries = limit.as_millis() / STEP.as_millis();
    (0..=tries).any(|_| {
        let held = done();
        if !held {
            std::thread::sleep(STEP);
        }
        held
    })
}

/// The fields of `/proc/<pid>/stat` the fleet reads.
struct Stat {
    /// One-letter process state; `Z` is a zombie.
    state: char,
    /// Session id.
    session: i32,
}

/// Reads `pid`'s state and session; `None` once it is gone.
fn stat(pid: Pid) -> Option<Stat> {
    let text = std::fs::read_to_string(format!("/proc/{}/stat", pid.as_raw_nonzero())).ok()?;
    // The command name may hold spaces and parentheses; the fields after its last `)` do not.
    let (_, rest) = text.rsplit_once(')')?;
    let mut fields = rest.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let session = fields.nth(2)?.parse().ok()?;
    Some(Stat { state, session })
}

/// `pid`'s argv joined by spaces; empty once it is gone.
fn cmdline(pid: Pid) -> String {
    std::fs::read(format!("/proc/{}/cmdline", pid.as_raw_nonzero()))
        .map(|raw| {
            raw.split(|&b| b == 0)
                .filter(|w| !w.is_empty())
                .map(String::from_utf8_lossy)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}
