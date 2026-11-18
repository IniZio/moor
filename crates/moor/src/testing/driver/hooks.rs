//! The fake's per-command exec knobs: outcomes that end one exec, and hooks that run before one.

use std::sync::Arc;

use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};

use moor_core::seam::proc::Deadline;
use moor_core::BoxHandle;

use super::script::ExecHook;
use super::{FakeDriver, State};
use crate::adapters::machine::link_down;

/// How many times an exec safe to repeat is asked while its link stays down, standing in for
/// the machine adapter's wait for the relink.
pub(crate) const RELINK_ASKS: usize = 5;

/// The per-command exec knobs.
#[derive(Default)]
pub(super) struct ExecKnobs {
    /// Outcomes that end the next `exec` whose argv mentions the needle, once each.
    ends_once: Vec<(String, ExecOutcome)>,
    /// Errors that fail the next `exec` whose argv mentions the needle, once each.
    errs_once: Vec<(String, DriverError)>,
    /// Errors that fail every `exec` whose argv mentions the needle.
    errs: Vec<(String, DriverError)>,
    /// Hooks run before any `exec` whose argv mentions the needle, outside the lock.
    hooks: Vec<(String, Arc<ExecHook>)>,
    /// Run before every `copy_out`, outside the lock.
    copy_out_hook: Option<Arc<ExecHook>>,
}

impl State {
    /// The error set up to fail the exec whose argv is `argv`; a once-error is used up.
    pub(super) fn exec_err(&mut self, argv: &str) -> Option<DriverError> {
        let knobs = &mut self.exec_knobs;
        if let Some(at) = knobs
            .errs_once
            .iter()
            .position(|(n, _)| argv.contains(n.as_str()))
        {
            return Some(knobs.errs_once.remove(at).1);
        }
        let hit = knobs.errs.iter().find(|(n, _)| argv.contains(n.as_str()));
        hit.map(|(_, e)| e.clone())
    }

    /// The outcome set up to end the next exec whose argv is `argv`, used up once served.
    pub(super) fn end_once(&mut self, argv: &str) -> Option<ExecOutcome> {
        let at = self
            .exec_knobs
            .ends_once
            .iter()
            .position(|(n, _)| argv.contains(n.as_str()))?;
        Some(self.exec_knobs.ends_once.remove(at).1)
    }
}

impl FakeDriver {
    /// Fails the next `exec` whose argv mentions `needle` with `err`, once.
    pub(crate) fn exec_fails_once_when(self, needle: &str, err: DriverError) -> Self {
        self.st()
            .exec_knobs
            .errs_once
            .push((needle.to_owned(), err));
        self
    }

    /// Fails every `exec` whose argv mentions `needle` with `err`.
    pub(crate) fn exec_fails_when(self, needle: &str, err: DriverError) -> Self {
        self.st().exec_knobs.errs.push((needle.to_owned(), err));
        self
    }

    /// Ends the next `exec` whose argv mentions `needle` with `outcome`, once.
    pub(crate) fn exec_ends_once_when(self, needle: &str, outcome: ExecOutcome) -> Self {
        self.st()
            .exec_knobs
            .ends_once
            .push((needle.to_owned(), outcome));
        self
    }

    /// Runs `spec`, asked again while a lost link fails one marked safe to repeat, as the machine
    /// adapter does, up to [`RELINK_ASKS`] asks.
    pub(super) fn exec_relinked(
        &self,
        handle: &BoxHandle,
        spec: &ExecSpec,
        deadline: Deadline,
    ) -> Result<ExecOutcome, DriverError> {
        let mut asks = 1;
        loop {
            let got = self.exec_once(handle, spec, deadline);
            match &got {
                Err(e) if spec.repeatable && asks < RELINK_ASKS && link_down(e) => asks += 1,
                _ => return got,
            }
        }
    }

    /// Runs `f` before every `exec` whose argv mentions `needle`, outside the Driver's lock.
    pub(crate) fn on_exec_of(&self, needle: &str, f: impl Fn() + Send + Sync + 'static) {
        self.st()
            .exec_knobs
            .hooks
            .push((needle.to_owned(), Arc::new(f)));
    }

    /// Runs the hook set up for every exec, then those set up for `spec`'s argv.
    pub(super) fn run_exec_hooks(&self, spec: &ExecSpec) {
        let argv = spec.argv.join(" ");
        let s = self.st();
        let named = s
            .exec_knobs
            .hooks
            .iter()
            .filter(|(n, _)| argv.contains(n.as_str()));
        let hooks: Vec<_> = s
            .on_exec
            .iter()
            .chain(named.map(|(_, h)| h))
            .cloned()
            .collect();
        drop(s);
        for hook in hooks {
            hook();
        }
    }

    /// Runs the hook set up for every `copy_out`.
    pub(super) fn run_copy_out_hook(&self) {
        let hook = self.st().exec_knobs.copy_out_hook.clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Runs `f` before every `copy_out`, outside the Driver's lock.
    pub(crate) fn on_copy_out(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().exec_knobs.copy_out_hook = Some(Arc::new(f));
    }
}
