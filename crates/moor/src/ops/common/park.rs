//! Parking and restoring a Box's Proxy tunnel on behalf of sleep and wake.
//!
//! The supervisor owns the tunnel; ops only ask through the store and wait for its answer.

use std::time::Duration;

use moor_core::{BoxId, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{Store, TunnelAck, TunnelWant};

/// How old a supervisor heartbeat may be before the supervisor counts as gone; matches the
/// supervisor's own lease.
const LEASE: Duration = Duration::from_secs(30);
/// Pause between looks at the acknowledgement.
const POLL: Duration = Duration::from_millis(200);
/// Time the supervisor gets to park: a few of its ticks.
const PARK_WAIT: Duration = Duration::from_secs(10);
/// Time the supervisor gets to bring the tunnel up; a reverse tunnel can be slow to become ready.
const RESTORE_WAIT: Duration = Duration::from_secs(90);

/// Asks the Box's Proxy to kill its tunnel and stop reviving it, and waits for it to confirm.
pub(crate) fn park_tunnel(cx: &Cx, store: &Store, box_id: BoxId) -> Result<(), CommandError> {
    match request(cx, store, box_id, TunnelWant::Parked, PARK_WAIT)? {
        Reply::NoSupervisor | Reply::Got(TunnelAck::Parked { .. }) => Ok(()),
        Reply::Got(TunnelAck::Failed { error, .. }) => Err(CommandError::new(
            ErrorKind::Failed,
            format!("the Proxy could not park the tunnel: {error}"),
        )),
        Reply::Got(_) => Err(unexpected()),
        Reply::TimedOut => Err(CommandError::new(
            ErrorKind::Failed,
            "the Proxy did not park the tunnel in time",
        )),
    }
}

/// Asks the Box's Proxy to bring its tunnel back up and waits until it is up.
///
/// `box_stayed_up` is true when the Box never stopped, so a stale session may still hold the port.
pub(crate) fn restore_tunnel(
    cx: &Cx,
    store: &Store,
    box_id: BoxId,
    box_stayed_up: bool,
) -> Result<(), CommandError> {
    let want = if box_stayed_up {
        TunnelWant::UpAwake
    } else {
        TunnelWant::Up
    };
    let reply = if store.proxy(box_id)?.is_some() && !supervisor_alive(cx, store, box_id)? {
        restart_supervisor(cx, box_id)?;
        ask(cx, store, box_id, want, RESTORE_WAIT)?
    } else {
        request(cx, store, box_id, want, RESTORE_WAIT)?
    };
    match reply {
        Reply::NoSupervisor | Reply::Got(TunnelAck::Up { .. }) => Ok(()),
        Reply::Got(TunnelAck::Failed { error, .. }) => Err(CommandError::new(
            ErrorKind::Failed,
            format!("the tunnel did not come back up: {error}"),
        )),
        Reply::Got(_) => Err(unexpected()),
        Reply::TimedOut => Err(CommandError::new(
            ErrorKind::Failed,
            "the Proxy did not bring the tunnel back up in time",
        )),
    }
}

/// What came of a tunnel request.
enum Reply {
    /// No live supervisor exists, so nothing was asked.
    NoSupervisor,
    /// The supervisor answered the request.
    Got(TunnelAck),
    /// The deadline passed without an answer.
    TimedOut,
}

/// Error for an answer that does not fit the request.
fn unexpected() -> CommandError {
    CommandError::new(
        ErrorKind::Internal,
        "the Proxy answered a tunnel request with the wrong state",
    )
}

/// Asks a live supervisor for `want`; with none, nothing is asked.
fn request(
    cx: &Cx,
    store: &Store,
    box_id: BoxId,
    want: TunnelWant,
    patience: Duration,
) -> Result<Reply, CommandError> {
    if !supervisor_alive(cx, store, box_id)? {
        return Ok(Reply::NoSupervisor);
    }
    ask(cx, store, box_id, want, patience)
}

/// Starts the Box's Proxy supervisor again from the spec its last start saved.
fn restart_supervisor(cx: &Cx, box_id: BoxId) -> Result<(), CommandError> {
    cx.proxy_host().restart(box_id).map_err(|e| {
        CommandError::new(
            ErrorKind::Failed,
            format!("the Proxy is gone and did not restart: {e}"),
        )
    })
}

/// Writes `want` and polls for the answer to that request until `patience` has passed.
fn ask(
    cx: &Cx,
    store: &Store,
    box_id: BoxId,
    want: TunnelWant,
    patience: Duration,
) -> Result<Reply, CommandError> {
    let seq = store.request_tunnel(box_id, want)?;
    let clock = cx.clock();
    let deadline = clock.instant() + patience;
    loop {
        if let Some(ack) = store.tunnel_ack(box_id)? {
            if ack.seq() == seq {
                return Ok(Reply::Got(ack));
            }
        }
        if clock.instant() >= deadline {
            return Ok(Reply::TimedOut);
        }
        clock.sleep(POLL);
    }
}

/// Whether a ready proxy row has a supervisor that heartbeat within the lease window.
fn supervisor_alive(cx: &Cx, store: &Store, box_id: BoxId) -> Result<bool, CommandError> {
    let Some(row) = store.proxy(box_id)? else {
        return Ok(false);
    };
    let (true, Some(_), Some(beat)) = (row.ready, row.lease_owner, row.lease_heartbeat) else {
        return Ok(false);
    };
    Ok(cx.clock().now() < beat.saturating_add(LEASE))
}

#[cfg(test)]
mod tests;
