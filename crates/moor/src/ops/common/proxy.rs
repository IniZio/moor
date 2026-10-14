//! Make sure a Task's proxy supervisor runs and is ready before an agent launches.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::engine::EngineError;
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Signal, Tool};
use moor_core::seam::rng::Rng;
use moor_core::{Secret, TaskId, Warning};

use crate::cx::Cx;
use crate::store::{ProxyRow, Reserve, Store, StoreError};

/// How long a supervisor lease stays live without a heartbeat.
///
/// Must equal the supervisor's own lease window.
pub(crate) const LEASE: Duration = Duration::from_secs(30);

/// Pause between two looks at the proxy row.
pub(crate) const POLL: Duration = Duration::from_millis(200);

/// How long to wait for the supervisor to become ready.
pub(crate) const WAIT: Duration = Duration::from_secs(30);

/// Log lines kept when a start fails.
pub(crate) const TAIL_LINES: usize = 20;

/// How long close waits for a signalled supervisor to remove its row.
const STOP_WAIT: Duration = Duration::from_secs(3);

/// Allowance for reading the origin URL.
const ORIGIN_DEADLINE: Duration = Duration::from_secs(5);

/// The base64url alphabet, without padding.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Fail the launch at the proxy step.
fn fail(reason: &str, log: &Path) -> EngineError {
    EngineError::Failed {
        step: "proxy",
        message: format!("{reason}\n{}", tail(log)),
    }
}

/// Map a store failure to a one-line launch failure.
fn store_failed(e: &StoreError, log: &Path) -> EngineError {
    fail(&format!("the proxy state could not be used: {e}"), log)
}

/// Whether the row's supervisor holds a live lease now.
fn live(cx: &Cx, row: &ProxyRow) -> bool {
    match (&row.lease_owner, row.lease_heartbeat) {
        (Some(_), Some(beat)) => {
            cx.clock()
                .now()
                .as_millis()
                .saturating_sub(beat.as_millis())
                < LEASE.as_millis().try_into().unwrap_or(i64::MAX)
        }
        _ => false,
    }
}

/// The address a ready row describes.
fn addr(row: &ProxyRow) -> Arc<ProxyAddr> {
    Arc::new(ProxyAddr {
        box_port: row.box_port,
        nonce: Secret::new(row.nonce.expose().to_vec()),
    })
}

/// Make sure the Task's proxy supervisor is running and ready; return its address.
///
/// A ready supervisor with a live lease is reused. Otherwise a new nonce is reserved and a
/// supervisor is started, unless one already holds a live lease. The nonce never appears in
/// arguments, environment, logs or errors.
pub(crate) fn ensure_proxy(
    cx: &Cx,
    store: &Store,
    task: TaskId,
) -> Result<Arc<ProxyAddr>, EngineError> {
    let state = cx.state_dir().map_err(|e| {
        fail(
            &format!("the state directory is not usable: {}", e.error),
            Path::new(""),
        )
    })?;
    let log = log_path(state, task);
    let row = store.proxy(task).map_err(|e| store_failed(&e, &log))?;
    if let Some(row) = row.filter(|r| r.ready && live(cx, r)) {
        return Ok(addr(&row));
    }

    let nonce = new_nonce(cx.rng());
    let reserved = store
        .reserve(task, &nonce, cx.rng(), LEASE)
        .map_err(|e| store_failed(&e, &log))?;
    let pid = match reserved {
        Reserve::Reserved(_) => {
            let spec = ProcSpec::new(Tool::MoorSelf, Deadline(WAIT)).args([
                "proxy",
                "serve",
                &task.get().to_string(),
            ]);
            let pid = cx
                .proc()
                .spawn_daemon(&spec, &log)
                .map_err(|e| fail(&format!("the proxy supervisor could not start: {e}"), &log))?;
            Some(pid)
        }
        Reserve::Leased { .. } => None,
    };

    let start = cx.clock().instant();
    loop {
        let row = store.proxy(task).map_err(|e| store_failed(&e, &log))?;
        if let Some(row) = &row {
            let lease = live(cx, row);
            if row.ready && lease {
                return Ok(addr(row));
            }
            if !lease && pid.is_some_and(|p| !cx.proc().alive(p)) {
                return Err(fail(
                    "the proxy supervisor exited before it was ready",
                    &log,
                ));
            }
        }
        if cx.clock().instant().saturating_duration_since(start) >= WAIT {
            return Err(fail("the proxy supervisor was not ready after 30s", &log));
        }
        cx.clock().sleep(POLL);
    }
}

/// Stop the Task's proxy supervisor and remove its row; never fails the caller.
///
/// SIGTERM goes to the supervisor's process group only while its lease is live, because a pid
/// alone may have been reused. The row is removed whether or not the supervisor exited in
/// time; a supervisor that outlasts the wait only produces a warning. No row is a no-op.
pub(crate) fn stop_proxy(cx: &Cx, store: &Store, task: TaskId) {
    let row = match store.proxy(task) {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => return warn_stop(cx, &format!("the proxy state could not be read: {e}")),
    };
    // Pids 0 and 1 would address every process or init's group.
    let pid = row.pid.filter(|p| *p > 1 && live(cx, &row));
    let exited = pid.is_none_or(|p| match signal_group(cx, p) {
        Sent::Delivered => wait_gone(cx, store, task),
        Sent::Gone => true,
        Sent::Failed => false,
    });
    if let Err(e) = store.remove_proxy(task) {
        return warn_stop(cx, &format!("the proxy row could not be removed: {e}"));
    }
    if !exited {
        warn_stop(cx, "the proxy supervisor did not exit within 3s");
    }
}

/// What happened to a SIGTERM sent to a process group.
enum Sent {
    /// The signal was delivered.
    Delivered,
    /// No such group: the supervisor is already gone.
    Gone,
    /// The group was refused or could not be signalled.
    Failed,
}

/// Send SIGTERM to the process group led by `pid`.
fn signal_group(cx: &Cx, pid: u32) -> Sent {
    match cx.proc().signal_group(pid, Signal::Term) {
        Ok(()) => Sent::Delivered,
        Err(ProcError::Spawn(e)) if e.kind() == io::ErrorKind::NotFound => Sent::Gone,
        Err(_) => Sent::Failed,
    }
}

/// Poll until the row is gone; false when it outlasts [`STOP_WAIT`].
fn wait_gone(cx: &Cx, store: &Store, task: TaskId) -> bool {
    let start = cx.clock().instant();
    loop {
        if !matches!(store.proxy(task), Ok(Some(_))) {
            return true;
        }
        if cx.clock().instant().saturating_duration_since(start) >= STOP_WAIT {
            return false;
        }
        cx.clock().sleep(POLL);
    }
}

/// Warn that stopping the proxy was incomplete; close still succeeds.
fn warn_stop(cx: &Cx, reason: &str) {
    cx.warn(Warning {
        code: "proxy_stop_failed".to_owned(),
        message: format!("{reason}; the task is closed anyway"),
    });
}

/// The `origin` remote URL of the worktree, when it has one.
pub(crate) fn origin(cx: &Cx, worktree: &Path) -> Option<String> {
    let spec = ProcSpec::new(Tool::Git, Deadline(ORIGIN_DEADLINE))
        .args(["remote", "get-url", "origin"])
        .cwd(worktree);
    let out = cx.proc().run(&spec).ok()?;
    let url = out.stdout.trim();
    (!url.is_empty()).then(|| url.to_owned())
}

/// Where the Task's proxy supervisor writes its log.
pub(crate) fn log_path(state_dir: &Path, task: TaskId) -> PathBuf {
    state_dir.join("proxy").join(format!("{}.log", task.get()))
}

/// A fresh nonce: 32 random bytes as base64url text.
pub(crate) fn new_nonce(rng: &dyn Rng) -> Secret {
    let bytes: Vec<u8> = (0..8).flat_map(|_| rng.u32().to_le_bytes()).collect();
    Secret::new(base64url(&bytes).into_bytes())
}

/// Encode bytes as base64url without padding.
pub(crate) fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let acc = chunk
            .iter()
            .chain(std::iter::repeat(&0))
            .take(3)
            .fold(0_u32, |a, b| (a << 8) | u32::from(*b));
        for shift in [18_u32, 12, 6, 0].iter().take(chunk.len() + 1) {
            let idx = usize::try_from((acc >> shift) & 63).unwrap_or(0);
            if let Some(c) = ALPHABET.get(idx) {
                out.push(char::from(*c));
            }
        }
    }
    out
}

/// The last lines of the log; empty when it cannot be read.
fn tail(log: &Path) -> String {
    let Ok(text) = fs::read_to_string(log) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    lines
        .iter()
        .skip(lines.len().saturating_sub(TAIL_LINES))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
