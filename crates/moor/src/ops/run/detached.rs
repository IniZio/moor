//! Follows a run detached in the Box: copies its log to the host byte for byte, cancels it in
//! the Box when its client leaves or its deadline passes, and reads how it ended.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{Cancelled, Driver, DriverError, FollowEnd, RunEnd, RunOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath, RunId};

use super::ready::Target;
use crate::cx::Cx;

/// Seconds a cancelled run gets between TERM and KILL.
const CANCEL_GRACE_SECS: u32 = 5;

/// Follows cut in a row with no new byte before the follow gives up.
const CUTS_MAX: u32 = 5;

/// Pause before following again after a cut.
const REFOLLOW_AFTER: Duration = Duration::from_secs(1);

/// How often the watcher checks for a client gone or a deadline passed.
const WATCH_EVERY: Duration = Duration::from_millis(100);

/// The longest one follow after a cancel waits for the rest of the log.
const DRAIN: Duration = Duration::from_secs(30);

/// Why moor cancelled a run in the Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stop {
    /// The client that asked for the run went away.
    Hangup,
    /// The run passed its deadline.
    Deadline,
}

/// How a followed run ended.
#[derive(Debug)]
pub(super) struct Ended {
    /// What the Box reported last, or why it could not be asked.
    pub(super) end: Result<RunEnd, String>,
    /// Why moor cancelled the run, and the cancel's error when it failed.
    pub(super) stop: Option<(Stop, Option<String>)>,
    /// Why the follow gave up before the run ended.
    pub(super) lost_follow: Option<String>,
}

impl Ended {
    /// The exit code the run reported, and whether it was cut short.
    pub(super) fn exit(&self) -> (Option<i32>, bool) {
        let exit = match &self.end {
            Ok(RunEnd {
                outcome: RunOutcome::Exited(code),
                ..
            }) => Some(i32::from(*code)),
            _ => None,
        };
        let cut = self.stop.is_some()
            || self.lost_follow.is_some()
            || !matches!(
                self.end,
                Ok(RunEnd {
                    outcome: RunOutcome::Exited(_),
                    cancelled: false,
                    ..
                })
            );
        (exit, cut)
    }

    /// Whether the record script left its moved-checkout marker.
    pub(super) fn moved(&self) -> bool {
        self.end.as_ref().is_ok_and(|e| e.moved)
    }

    /// Why the run failed beyond its own exit code, if it did.
    pub(super) fn error(&self) -> Option<String> {
        let mut why = Vec::new();
        if let Some((Stop::Deadline, _)) = self.stop {
            why.push("the run passed its deadline and was cancelled in the Box".to_owned());
        }
        if let Some((_, Some(e))) = &self.stop {
            why.push(format!("the cancel in the Box failed: {e}"));
        }
        why.extend(self.lost_follow.clone());
        if let Err(e) = &self.end {
            why.push(format!("cannot read how the run ended: {e}"));
        }
        (!why.is_empty()).then(|| why.join("; "))
    }
}

/// Follows run `id` until it ends, writing its log from byte `from` to `out`.
///
/// The run is cancelled in the Box once the request is cancelled or `until` passes.
pub(super) fn follow(
    cx: &Cx,
    target: &Target,
    workdir: Option<&BoxPath>,
    id: RunId,
    until: Ts,
    (from, out): (u64, &mut dyn Write),
) -> Ended {
    let flag = cx.env().cancel();
    let run = Run {
        driver: cx.driver(),
        clock: cx.clock(),
        cancelled: &|| flag.is_cancelled(),
        handle: &target.handle,
        id: id.get(),
        until,
    };
    let done = AtomicBool::new(false);
    let (lost_follow, stop) = thread::scope(|s| {
        let watcher = s.spawn(|| run.watch(&done));
        let copied = run.copy(from, out);
        done.store(true, Ordering::SeqCst);
        let stop = watcher.join().unwrap_or(None);
        let lost = match copied {
            Copied::Ended => None,
            Copied::Stopped(offset) => {
                let _ = run
                    .driver
                    .follow_run(run.handle, run.id, offset, out, Deadline(DRAIN));
                None
            }
            Copied::Lost(why) => Some(match run.cancel_in_box() {
                Ok(_) => why,
                Err(e) => format!("{why}; the cancel in the Box failed: {e}"),
            }),
        };
        (lost, stop)
    });
    let end = run
        .driver
        .end_run(run.handle, run.id, workdir)
        .map_err(|e| e.to_string());
    Ended {
        end,
        stop,
        lost_follow,
    }
}

/// Where the copy of the log stopped.
enum Copied {
    /// The run ended and its whole log was copied.
    Ended,
    /// A cancel or the deadline cut the follow at this offset.
    Stopped(u64),
    /// The follow failed and the run may still be going.
    Lost(String),
}

/// A run being followed, with what may stop it.
struct Run<'a> {
    /// Reaches the Box.
    driver: &'a dyn Driver,
    /// Tells when the deadline passes.
    clock: &'a dyn Clock,
    /// True once the client that asked for the run is gone.
    cancelled: &'a (dyn Fn() -> bool + Sync),
    /// The Box the run is in.
    handle: &'a BoxHandle,
    /// The run's id, which names its dir in the Box.
    id: i64,
    /// When the run's deadline passes.
    until: Ts,
}

impl Run<'_> {
    /// Copies the log to `out` from `from`, following again after a cut.
    fn copy(&self, from: u64, out: &mut dyn Write) -> Copied {
        let mut from = from;
        let mut cuts = 0;
        loop {
            let left = Deadline(self.remaining());
            let followed = match self
                .driver
                .follow_run(self.handle, self.id, from, out, left)
            {
                Ok(f) => f,
                Err(e) => return Copied::Lost(format!("following run {} failed: {e}", self.id)),
            };
            let moved = followed.offset > from;
            from = followed.offset;
            match followed.end {
                FollowEnd::Exited | FollowEnd::Lost => return Copied::Ended,
                FollowEnd::Cut if self.stopping().is_some() => return Copied::Stopped(from),
                FollowEnd::Cut => {}
            }
            cuts = if moved { 0 } else { cuts + 1 };
            if cuts >= CUTS_MAX {
                return Copied::Lost(format!(
                    "the follow of run {} was cut {CUTS_MAX} times in a row",
                    self.id
                ));
            }
            thread::sleep(REFOLLOW_AFTER);
        }
    }

    /// Waits until `done`, cancelling the run in the Box first if the request is cancelled or
    /// the deadline passes; returns why it cancelled and the cancel's error.
    fn watch(&self, done: &AtomicBool) -> Option<(Stop, Option<String>)> {
        loop {
            // Checked before `done`, so a follow that ended because of a stop still cancels.
            if let Some(stop) = self.stopping() {
                return Some((stop, self.cancel_in_box().err().map(|e| e.to_string())));
            }
            if done.load(Ordering::SeqCst) {
                return None;
            }
            thread::sleep(WATCH_EVERY);
        }
    }

    /// Stops the run's process group in the Box.
    fn cancel_in_box(&self) -> Result<Cancelled, DriverError> {
        self.driver
            .cancel_run(self.handle, self.id, CANCEL_GRACE_SECS)
    }

    /// Why the run must stop now, if it must.
    fn stopping(&self) -> Option<Stop> {
        if (self.cancelled)() {
            Some(Stop::Hangup)
        } else if self.clock.now() >= self.until {
            Some(Stop::Deadline)
        } else {
            None
        }
    }

    /// Time left before the deadline.
    fn remaining(&self) -> Duration {
        let ms = self
            .until
            .as_millis()
            .saturating_sub(self.clock.now().as_millis());
        Duration::from_millis(u64::try_from(ms).unwrap_or(0))
    }
}
