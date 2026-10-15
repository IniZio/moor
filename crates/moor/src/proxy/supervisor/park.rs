//! Parking the reverse tunnel while the Box sleeps and reopening it on wake.

use moor_core::seam::driver::TunnelHandle;
use moor_core::TaskId;

use super::run::{say, Running};
use super::Deps;
use crate::store::{TunnelAck, TunnelRequest, TunnelWant};

/// Stands in for the tunnel while parked; it never runs, so it is never alive.
struct Parked;

impl TunnelHandle for Parked {
    fn pid(&self) -> u32 {
        0
    }

    fn alive(&self) -> bool {
        false
    }
}

/// The dead stand-in that holds the tunnel slot while parked.
pub(super) fn parked_tunnel() -> Box<dyn TunnelHandle> {
    Box::new(Parked)
}

/// Whether a starting supervisor leaves the tunnel closed: the Box is asleep or the latest request is Parked.
///
/// A store error is reported and the Box state decides alone.
pub(super) fn starts_parked(deps: &Deps<'_>, task: TaskId, asleep: bool) -> bool {
    match deps.store.tunnel_request(task) {
        Ok(request) => asleep || request.is_some_and(|r| r.want == TunnelWant::Parked),
        Err(err) => {
            say(
                &deps.stderr,
                format_args!("cannot read the tunnel request: {err}"),
            );
            asleep
        }
    }
}

impl Running<'_> {
    /// Acts on a tunnel request that has not been answered yet.
    ///
    /// Store errors are reported and skipped: they must not stop the proxy.
    pub(super) fn follow_request(&self) {
        let Some(request) = self.pending_request() else {
            return;
        };
        match request.want {
            TunnelWant::Parked => {
                if !self.parked.get() {
                    // Replacing the handle drops the old one, which kills the tunnel.
                    *self.tunnel.borrow_mut() = parked_tunnel();
                    self.parked.set(true);
                }
                self.answer(&TunnelAck::Parked { seq: request.seq });
            }
            TunnelWant::Up if self.parked.get() => self.reopen(request.seq),
            TunnelWant::Up => {
                // A dead tunnel falls through to the restart path and is answered once it is up.
                if self.tunnel.borrow().alive() {
                    self.answer(&TunnelAck::Up { seq: request.seq });
                }
            }
        }
    }

    /// The current request unless it was already answered.
    fn pending_request(&self) -> Option<TunnelRequest> {
        let store = self.deps.store;
        let sink = &self.deps.stderr;
        let request = match store.tunnel_request(self.task) {
            Ok(request) => request?,
            Err(err) => {
                say(sink, format_args!("cannot read the tunnel request: {err}"));
                return None;
            }
        };
        match store.tunnel_ack(self.task) {
            Ok(Some(ack)) if ack.seq() == request.seq => None,
            Ok(_) => Some(request),
            Err(err) => {
                say(sink, format_args!("cannot read the tunnel answer: {err}"));
                None
            }
        }
    }

    /// Opens one new tunnel on the same ports and answers `seq`.
    fn reopen(&self, seq: u64) {
        let opened = self.deps.driver.reverse_tunnel(
            &self.handle,
            self.box_port,
            self.live.port(),
            &self.worktree,
        );
        // A failed open leaves the dead stand-in, so the restart path takes over.
        self.parked.set(false);
        match opened {
            Ok(tunnel) => {
                *self.tunnel.borrow_mut() = tunnel;
                self.answer(&TunnelAck::Up { seq });
            }
            Err(err) => {
                say(
                    &self.deps.stderr,
                    format_args!("cannot reopen the tunnel: {err}"),
                );
                self.answer(&TunnelAck::Failed {
                    seq,
                    error: err.to_string(),
                });
            }
        }
    }

    /// Stores the answer, reporting a failure.
    fn answer(&self, ack: &TunnelAck) {
        if let Err(err) = self.deps.store.ack_tunnel(self.task, ack) {
            say(
                &self.deps.stderr,
                format_args!("cannot answer the tunnel request: {err}"),
            );
        }
    }
}

#[cfg(test)]
mod tests;
