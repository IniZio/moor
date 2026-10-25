//! A Box's shared ssh connection.
//!
//! moor's short ssh calls to one Box (file reads and writes, step batches, file streams) ride
//! on one OpenSSH control master, so a Box probed every few seconds pays for one connect, not
//! one per probe. The target words a master was opened with live under moor's state dir and
//! its socket in a short private directory, so every moor process and every freshly wired
//! driver reuses them without a new lookup. A saved target is kept until a call on it fails:
//! every lookup touches the Box's lease, so a Box that is only looked at must not cause one. The agent's pane keeps its own connection, so
//! ending a master never ends the agent.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::time::Duration;

use moor_core::name::BoxHandle;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::DriverError;
use moor_core::seam::proc::{Deadline, ProcError, ProcOutput, ProcSpec, Tool};
use moor_core::Warning;
use serde::{Deserialize, Serialize};

use super::errors::{one_line, transport_loss, EXIT_SSH};
use super::{pin, Crabbox};
use crate::adapters::sockets::{private_dir, short_hash};

/// Seconds a master stays up after its last call, so a Box used within that time pays for no
/// connect.
const PERSIST_SECS: u32 = 600;
/// Time allowed for a request to a master; it goes over the local socket only.
const CONTROL_DEADLINE: Duration = Duration::from_secs(5);
/// Bytes of a master's log that go into an error message.
const LOG_TAIL: u64 = 300;
/// Size past which a master's log is emptied before the next call.
const LOG_MAX: u64 = 16 * 1024;

/// Where the shared connections of all Boxes live.
pub(super) struct Dirs {
    /// The state dir; it scopes socket names, so two state dirs never share a master.
    state: PathBuf,
    /// The private directory holding the control sockets.
    sockets: PathBuf,
    /// The uid that must own `sockets`: the uid moor runs as.
    owner: u32,
    /// Seconds a master stays up after its last call.
    persist: u32,
}

/// Where the shared connection of one Box lives.
pub(super) struct Link {
    /// The master's control socket.
    socket: PathBuf,
    /// The saved target, as JSON.
    target: PathBuf,
    /// Where the master writes its errors: ssh sends a persisted master's stderr to /dev/null.
    log: PathBuf,
    /// Seconds the master stays up after its last call.
    persist: u32,
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
        self.leases = state_dir.as_ref().map(|state| state.join("leases"));
        self.links = state_dir.map(|state| Dirs {
            state,
            sockets,
            owner,
            persist: PERSIST_SECS,
        });
        self
    }

    /// Keeps each master up for `secs` after its last call, in place of [`PERSIST_SECS`].
    #[cfg(test)]
    pub(super) const fn with_persist(mut self, secs: u32) -> Self {
        if let Some(dirs) = self.links.as_mut() {
            dirs.persist = secs;
        }
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
            log: dirs.sockets.join(format!("{scoped}.log")),
            socket: dirs.sockets.join(scoped),
            target: dirs.state.join("ssh").join(format!("{name}.target")),
            persist: dirs.persist,
        })
    }
}

impl Link {
    /// The saved target words, however old: they hold until a call on them fails.
    pub(super) fn saved(&self) -> Option<Vec<String>> {
        let text = fs::read_to_string(&self.target).ok()?;
        let saved = serde_json::from_str::<Saved>(&text).ok()?;
        (!saved.words.is_empty()).then_some(saved.words)
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
            format!("ControlPersist={}", self.persist),
            "-o".to_owned(),
            format!("ControlPath={}", self.socket.display()),
            "-E".to_owned(),
            self.log.display().to_string(),
            "-o".to_owned(),
            "LogLevel=ERROR".to_owned(),
        ]
    }

    /// Empties the log when the next call opens a master, or when the log has grown past
    /// [`LOG_MAX`], and makes sure only this user can read it. A failure only costs the tail.
    fn prepare_log(&self) {
        let grown = fs::metadata(&self.log).is_ok_and(|m| m.len() > LOG_MAX);
        if self.socket.exists() && !grown {
            return;
        }
        let _ = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&self.log)
            .and_then(|f| f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600)));
    }

    /// The last bytes the master logged, as one line; empty when it logged nothing or the log is
    /// a symlink, which only someone else could have planted.
    fn log_tail(&self) -> String {
        let read = OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed())
            .open(&self.log)
            .and_then(|mut f| {
                let len = f.metadata()?.len();
                f.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL)))?;
                let mut bytes = Vec::new();
                f.read_to_end(&mut bytes)?;
                Ok(bytes)
            });
        one_line(&String::from_utf8_lossy(&read.unwrap_or_default()))
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
/// socket dir, so this makes it first. Arguments that do not pin the Box's host key are
/// refused.
pub(super) fn args(
    cb: &Crabbox,
    handle: &BoxHandle,
    target: Vec<String>,
) -> Result<Vec<String>, DriverError> {
    let args = match cb.link(handle).filter(|_| cb.unshared().is_none()) {
        Some(link) => {
            link.prepare_log();
            let mut args = link.options();
            args.extend(target);
            args
        }
        None => target,
    };
    pin::check(cb, &args)?;
    Ok(args)
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
/// why; moor's short calls never exit 255 themselves. The error then carries what the
/// master logged, unless the sockets dir is not private: then the log may be someone else's.
pub(super) fn lost(cb: &Crabbox, handle: &BoxHandle, e: &ProcError) -> Option<DriverError> {
    match e {
        ProcError::NonZero {
            code: EXIT_SSH,
            stderr_tail,
            ..
        } if stderr_tail.trim().is_empty() => Some(DriverError::Transport {
            stderr_tail: cb
                .link(handle)
                .filter(|_| cb.unshared().is_none())
                .map(|l| l.log_tail())
                .unwrap_or_default(),
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
        let _ = fs::remove_file(&link.log);
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
