//! A Box's shared ssh connection.
//!
//! moor's short ssh calls to one Box (file reads and writes, step batches, file streams) ride
//! on one OpenSSH control master, so a Box probed every few seconds pays for one connect, not
//! one per probe. The target words a master was opened with live under moor's state dir and
//! its socket in a short private directory, so every moor process and every freshly wired
//! driver reuses them without a new lookup. The agent's pane keeps its own connection, so
//! ending a master never ends the agent.

use std::fmt::Write as _;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::name::BoxHandle;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{Deadline, ProcError, ProcOutput, ProcSpec, Tool};
use moor_core::Warning;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::errors::{transport_loss, EXIT_SSH};
use super::Crabbox;

/// Seconds a master stays up after its last call: long enough to bridge probe ticks and
/// back-to-back commands, short enough that a stopped moor leaves no connection open for long.
const PERSIST_SECS: u32 = 120;
/// Milliseconds a saved target stands in for a lookup. A lookup also touches the Box's lease,
/// so a busy Box still gets one well inside crabbox's 30 min default idle timeout.
const TARGET_TTL_MS: i64 = 10 * 60 * 1000;
/// Time allowed for a request to a master; it goes over the local socket only.
const CONTROL_DEADLINE: Duration = Duration::from_secs(5);

/// Where moor's control sockets live: `/tmp/moor-<uid>`. ssh first binds the socket path plus
/// a 17-byte suffix, and macOS allows 103 bytes, so a socket under a deep state dir would fail
/// every call; this path stays short wherever the state dir is.
pub(crate) fn socket_dir() -> PathBuf {
    PathBuf::from(format!("/tmp/moor-{}", rustix::process::getuid().as_raw()))
}

/// Where the shared connections of all Boxes live.
pub(super) struct Dirs {
    /// The state dir; it scopes socket names, so two state dirs never share a master.
    state: PathBuf,
    /// The private directory holding the control sockets.
    sockets: PathBuf,
    /// The uid that must own `sockets`: the uid moor runs as.
    owner: u32,
}

/// Where the shared connection of one Box lives.
pub(super) struct Link {
    /// The master's control socket.
    socket: PathBuf,
    /// The saved target, as JSON.
    target: PathBuf,
}

/// The target words the master connects with, and when they were looked up.
#[derive(Serialize, Deserialize)]
struct Saved {
    /// Lookup time, in milliseconds on moor's clock.
    at: i64,
    /// The ssh arguments that reach the Box.
    words: Vec<String>,
}

impl Crabbox {
    /// Keeps each Box's shared ssh connection: its saved target under `state_dir`, its socket
    /// in `sockets`. Without a state dir, every call to a Box connects on its own.
    pub(crate) fn with_state_dir(mut self, state_dir: Option<PathBuf>, sockets: PathBuf) -> Self {
        let owner = rustix::process::getuid().as_raw();
        self.links = state_dir.map(|state| Dirs {
            state,
            sockets,
            owner,
        });
        self
    }

    /// Why calls to a Box cannot share a connection; none when they can or when moor keeps
    /// no state.
    fn unshared(&self) -> Option<String> {
        let dirs = self.links.as_ref()?;
        private_dir(&dirs.sockets, dirs.owner).err()
    }

    /// [`Crabbox::unshared`] as a warning.
    pub(crate) fn link_warning(&self) -> Option<Warning> {
        let reason = self.unshared()?;
        Some(Warning {
            code: "ssh_not_shared".to_owned(),
            message: format!("every ssh call to a Box opens its own connection: {reason}"),
        })
    }

    /// The shared connection of the Box behind `handle`; none without a state dir.
    pub(super) fn link(&self, handle: &BoxHandle) -> Option<Link> {
        let dirs = self.links.as_ref()?;
        let name = short_hash(&[handle.as_str().as_bytes()]);
        let scoped = short_hash(&[
            dirs.state.as_os_str().as_bytes(),
            handle.as_str().as_bytes(),
        ]);
        Some(Link {
            socket: dirs.sockets.join(scoped),
            target: dirs.state.join("ssh").join(format!("{name}.target")),
        })
    }
}

/// The first 8 bytes of the SHA-256 of `parts`, each ended by a NUL byte, in hex.
fn short_hash(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Creates `dir` for control sockets, or checks the one there: it must be a directory of
/// `owner` that nobody else can enter, since any process that reaches a socket can use its
/// master.
fn private_dir(dir: &Path, owner: u32) -> Result<(), String> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != ErrorKind::AlreadyExists => {
            return Err(format!("cannot create {}: {e}", dir.display()));
        }
        _ => {}
    }
    let meta =
        fs::symlink_metadata(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let own = meta.is_dir() && meta.uid() == owner && meta.mode() & 0o777 == 0o700;
    if own {
        Ok(())
    } else {
        Err(format!(
            "{} is not a directory that only this user can enter",
            dir.display()
        ))
    }
}

impl Link {
    /// The saved target words when they were looked up less than [`TARGET_TTL_MS`] before `now`.
    pub(super) fn saved(&self, now: Ts) -> Option<Vec<String>> {
        let text = fs::read_to_string(&self.target).ok()?;
        let saved = serde_json::from_str::<Saved>(&text).ok()?;
        let age = now.as_millis().checked_sub(saved.at)?;
        let fresh = (0..TARGET_TTL_MS).contains(&age) && !saved.words.is_empty();
        fresh.then_some(saved.words)
    }

    /// Saves `words`, looked up at `now`, for later calls; a failed save only costs a lookup.
    pub(super) fn save(&self, words: &[String], now: Ts) {
        let saved = Saved {
            at: now.as_millis(),
            words: words.to_vec(),
        };
        let tmp = self
            .target
            .with_extension(format!("{}.tmp", std::process::id()));
        let dir = self.target.parent().unwrap_or(&self.target);
        let written = DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .and_then(|()| {
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&tmp)
            })
            .and_then(|mut f| f.write_all(&serde_json::to_vec(&saved).unwrap_or_default()))
            .and_then(|()| fs::rename(&tmp, &self.target));
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }

    /// Options that put a call on this connection, opening it when it is down. They go before
    /// the target words, because ssh keeps the first value it reads for an option.
    fn options(&self) -> Vec<String> {
        vec![
            "-o".to_owned(),
            "ControlMaster=auto".to_owned(),
            "-o".to_owned(),
            format!("ControlPersist={PERSIST_SECS}"),
            "-o".to_owned(),
            format!("ControlPath={}", self.socket.display()),
        ]
    }

    /// Sends `request` (`exit` or `stop`) to the master; false when no master answered.
    fn control(&self, cb: &Crabbox, request: &str) -> bool {
        if !self.socket.exists() {
            return false;
        }
        let spec = ProcSpec::new(Tool::Ssh, Deadline(CONTROL_DEADLINE))
            .args(["-F", "none", "-o"])
            .arg(format!("ControlPath={}", self.socket.display()))
            .args(["-O", request, "box"]);
        cb.proc.run(&spec).is_ok()
    }
}

/// The ssh arguments for a short call to the Box: the shared-connection options, then
/// `target`. Without a usable link the call connects on its own. ssh does not create the
/// socket dir, so this makes it first.
pub(super) fn args(cb: &Crabbox, handle: &BoxHandle, target: Vec<String>) -> Vec<String> {
    match cb.link(handle).filter(|_| cb.unshared().is_none()) {
        Some(link) => {
            let mut args = link.options();
            args.extend(target);
            args
        }
        None => target,
    }
}

/// Runs `spec`, a short call to the Box, and handles a failure as [`failed`] does.
pub(super) fn run(
    cb: &Crabbox,
    handle: &BoxHandle,
    spec: &ProcSpec,
) -> Result<ProcOutput, ProcError> {
    let ran = cb.proc.run(spec);
    if let Err(e) = &ran {
        failed(cb, handle, e);
    }
    ran
}

/// After a short call to the Box failed with `e`. A timeout tells the master to stop taking
/// calls, since it may hang on a stalled connection: its other calls finish, and the next call
/// opens a fresh master. An ssh failure forgets the saved target, which may be stale; a master
/// it lost is replaced by ssh on the next call. Any other exit is the remote command's own.
pub(super) fn failed(cb: &Crabbox, handle: &BoxHandle, e: &ProcError) {
    let Some(link) = cb.link(handle) else {
        return;
    };
    match e {
        ProcError::Timeout { .. } => {
            link.control(cb, "stop");
        }
        ProcError::NonZero { code: EXIT_SSH, .. } => link.forget(),
        _ => {}
    }
}

/// The lost connection behind a failed short call, when that is what failed. A call whose
/// shared master lost the network exits 255 and prints nothing, since only the master saw
/// why; moor's short calls never exit 255 themselves.
pub(super) fn lost(e: &ProcError) -> Option<DriverError> {
    match e {
        ProcError::NonZero {
            code: EXIT_SSH,
            stderr_tail,
            ..
        } if stderr_tail.trim().is_empty() => Some(DriverError::Transport {
            stderr_tail: String::new(),
        }),
        _ => transport_loss(e),
    }
}

/// Ends the shared connection of the Box behind `handle`, with every call on it, and forgets
/// its saved target.
pub(super) fn close(cb: &Crabbox, handle: &BoxHandle) {
    if let Some(link) = cb.link(handle) {
        if !link.control(cb, "exit") {
            let _ = fs::remove_file(&link.socket);
        }
        link.forget();
    }
}

impl Link {
    /// Forgets the saved target.
    fn forget(&self) {
        let _ = fs::remove_file(&self.target);
    }
}

#[cfg(test)]
mod tests;
