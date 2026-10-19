//! Resume registration: tells herdr how to restart an agent session after a restart.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use moor_core::activity::PaneStatus;
use moor_core::seam::clock::Clock;
use moor_core::seam::surface::{PaneRef, ResumeOutcome, ResumeReg, SurfaceError};

use super::Herdr;

/// Source name herdr files moor's reports under.
const SOURCE: &str = "moor";
/// Codes herdr answers when it declines a resume command.
const DECLINED: [&str; 2] = ["invalid_resume_argv", "resume_not_accepted"];

/// herdr's word for a status.
const fn state_word(state: PaneStatus) -> &'static str {
    match state {
        PaneStatus::Working => "working",
        PaneStatus::Idle => "idle",
        PaneStatus::Blocked => "blocked",
        PaneStatus::Unknown => "unknown",
    }
}

/// The decline code in `e`, when herdr refused the resume command.
fn declined(e: &SurfaceError) -> Option<&'static str> {
    let SurfaceError::Failed { summary } = e else {
        return None;
    };
    DECLINED
        .into_iter()
        .find(|code| summary.strip_prefix("herdr ") == Some(code))
}

impl Herdr {
    /// Takes report sequence numbers from `clock`, so none falls below the time in microseconds.
    ///
    /// herdr ignores a report whose number is not above the last it saw, so a long-lived process
    /// must not fall behind a newer one.
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// The next report sequence number: above the one before, and not below the clock.
    fn next_seq(&self) -> String {
        let floor = self.clock.as_ref().map_or(0, |c| {
            u64::try_from(c.now().as_millis())
                .unwrap_or_default()
                .saturating_mul(1000)
        });
        let mut next = 0;
        let _ = self
            .seq
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |p| {
                next = p.saturating_add(1).max(floor);
                Some(next)
            });
        next.to_string()
    }

    /// Reports the pane's agent, then registers the command that resumes it.
    pub(super) fn register(
        &self,
        pane: &PaneRef,
        reg: &ResumeReg,
    ) -> Result<ResumeOutcome, SurfaceError> {
        let state = self.next_seq();
        self.call(&[
            "pane",
            "report-agent",
            pane.as_str(),
            "--source",
            SOURCE,
            "--agent",
            &reg.agent,
            "--state",
            state_word(reg.state),
            "--seq",
            &state,
        ])?;
        let seq = self.next_seq();
        let out = self.call(&[
            "pane",
            "report-agent-session",
            pane.as_str(),
            "--source",
            SOURCE,
            "--agent",
            &reg.agent,
            "--seq",
            &seq,
            "--agent-session-id",
            &reg.session_id,
            "--",
            "moor",
            "start",
            reg.box_name.as_str(),
            "--resume",
        ]);
        match out {
            Ok(_) => Ok(ResumeOutcome::Registered),
            Err(e) => declined(&e)
                .map(|code| ResumeOutcome::Rejected(code.to_owned()))
                .ok_or(e),
        }
    }
}

#[cfg(test)]
mod tests;
