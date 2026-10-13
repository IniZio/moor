//! In-memory `Engine` for unit tests of ops.
#![allow(
    dead_code,
    clippy::significant_drop_tightening,
    reason = "test fake; not yet used by ops"
)]

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::activity::{HookRead, PaneRead, Probes};
use moor_core::seam::engine::{Engine, EngineError, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::PaneRef;

#[derive(Default)]
struct State {
    probes: HashMap<String, Probes>,
    fail_probe: HashMap<String, EngineError>,
    counts: HashMap<String, usize>,
    panes: HashMap<String, Option<PaneRef>>,
}

/// Scripted Engine: probes are served per session id and counted.
#[derive(Default)]
pub(crate) struct FakeEngine {
    state: Mutex<State>,
}

impl FakeEngine {
    /// An Engine whose sessions all probe as `Absent` until scripted.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every probe of `session` returns `p`.
    pub(crate) fn probes(self, session: &str, p: Probes) -> Self {
        self.st().probes.insert(session.to_owned(), p);
        self
    }

    /// The next probe of `session` fails once with `e`.
    pub(crate) fn fail_probe(self, session: &str, e: EngineError) -> Self {
        self.st().fail_probe.insert(session.to_owned(), e);
        self
    }

    /// How many times `session` was probed, failures included.
    pub(crate) fn probe_count(&self, session: &str) -> usize {
        self.st().counts.get(session).copied().unwrap_or(0)
    }

    /// The pane the latest probe of `session` was given; none if never probed.
    pub(crate) fn probed_pane(&self, session: &str) -> Option<PaneRef> {
        self.st().panes.get(session).cloned().flatten()
    }
}

impl Engine for FakeEngine {
    fn probe(&self, s: &SessionRef, _deadline: Deadline) -> Result<Probes, EngineError> {
        let mut st = self.st();
        *st.counts.entry(s.id.clone()).or_default() += 1;
        st.panes.insert(s.id.clone(), s.pane.clone());
        if let Some(e) = st.fail_probe.remove(&s.id) {
            return Err(e);
        }
        Ok(st.probes.get(&s.id).cloned().unwrap_or(Probes {
            hook: HookRead::Absent,
            pane: PaneRead::Absent,
        }))
    }
}

#[cfg(test)]
mod tests;
