//! In-memory `Engine` for unit tests of ops.
#![allow(
    dead_code,
    clippy::significant_drop_tightening,
    reason = "test fake; not yet used by ops"
)]

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use moor_core::activity::{HookRead, PaneRead, Probes};
use moor_core::seam::agent_profile::{FirstRunPrompt, PromptMatch};
use moor_core::seam::engine::{Engine, EngineError, LaunchReq, SessionRef, Steer, SteerOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::PaneRef;

#[derive(Default)]
struct State {
    probes: HashMap<String, Probes>,
    queued: HashMap<String, VecDeque<Probes>>,
    fail_probe: HashMap<String, EngineError>,
    counts: HashMap<String, usize>,
    panes: HashMap<String, Option<PaneRef>>,
    launch_results: VecDeque<Result<SessionRef, EngineError>>,
    launches: Vec<LaunchReq>,
    prompts: Vec<(String, FirstRunPrompt)>,
    steer_results: VecDeque<Result<SteerOutcome, EngineError>>,
    steers: Vec<(String, Steer)>,
    transcripts: Vec<String>,
    transcript_text: Option<String>,
    later_texts: VecDeque<String>,
    fail_transcript: Option<EngineError>,
    on_transcript: Option<Arc<dyn Fn() + Send + Sync>>,
    egress_hosts: &'static [&'static str],
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

    /// The hosts the agent reports it needs.
    pub(crate) fn egress_hosts(self, hosts: &'static [&'static str]) -> Self {
        self.st().egress_hosts = hosts;
        self
    }

    /// Every probe of `session` returns `p`.
    pub(crate) fn probes(self, session: &str, p: Probes) -> Self {
        self.st().probes.insert(session.to_owned(), p);
        self
    }

    /// The next probe of `session` returns `p`, ahead of the fixed reading.
    pub(crate) fn then_probes(self, session: &str, p: Probes) -> Self {
        self.st()
            .queued
            .entry(session.to_owned())
            .or_default()
            .push_back(p);
        self
    }

    /// The next probe of `session` fails once with `e`.
    pub(crate) fn fail_probe(self, session: &str, e: EngineError) -> Self {
        self.st().fail_probe.insert(session.to_owned(), e);
        self
    }

    /// The next launch returns `r`; launches past the scripted ones succeed with pane `p-1`.
    pub(crate) fn launch_result(self, r: Result<SessionRef, EngineError>) -> Self {
        self.st().launch_results.push_back(r);
        self
    }

    /// A pane tail containing `text` shows `prompt`.
    pub(crate) fn prompt(self, text: &str, prompt: FirstRunPrompt) -> Self {
        self.st().prompts.push((text.to_owned(), prompt));
        self
    }

    /// The next steer returns `r`; steers past the scripted ones succeed without a retry.
    pub(crate) fn steer_result(self, r: Result<SteerOutcome, EngineError>) -> Self {
        self.st().steer_results.push_back(r);
        self
    }

    /// Every steer so far as the session id and what was sent, in call order.
    pub(crate) fn steers(&self) -> Vec<(String, Steer)> {
        self.st().steers.clone()
    }

    /// Every transcript request writes `text` to its destination.
    pub(crate) fn transcript_text(self, text: &str) -> Self {
        self.st().transcript_text = Some(text.to_owned());
        self
    }

    /// The next transcript request without a queued text of its own writes `text`, ahead of the fixed one.
    pub(crate) fn then_transcript_text(self, text: &str) -> Self {
        self.st().later_texts.push_back(text.to_owned());
        self
    }

    /// The next transcript request fails once with `e`.
    pub(crate) fn fail_transcript(self, e: EngineError) -> Self {
        self.st().fail_transcript = Some(e);
        self
    }

    /// The next transcript request fails once with `e`, set on an Engine already shared.
    pub(crate) fn fail_next_transcript(&self, e: EngineError) {
        self.st().fail_transcript = Some(e);
    }

    /// Runs `f` inside every transcript request, before it is served.
    pub(crate) fn on_transcript(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().on_transcript = Some(Arc::new(f));
    }

    /// The session ids transcripts were requested for, in call order.
    pub(crate) fn transcripts(&self) -> Vec<String> {
        self.st().transcripts.clone()
    }

    /// Every launch request so far, in call order.
    pub(crate) fn launches(&self) -> Vec<LaunchReq> {
        self.st().launches.clone()
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
    fn egress_hosts(&self) -> &'static [&'static str] {
        self.st().egress_hosts
    }

    fn probe(&self, s: &SessionRef, _deadline: Deadline) -> Result<Probes, EngineError> {
        let mut st = self.st();
        *st.counts.entry(s.id.clone()).or_default() += 1;
        st.panes.insert(s.id.clone(), s.pane.clone());
        if let Some(e) = st.fail_probe.remove(&s.id) {
            return Err(e);
        }
        if let Some(p) = st.queued.get_mut(&s.id).and_then(VecDeque::pop_front) {
            return Ok(p);
        }
        Ok(st.probes.get(&s.id).cloned().unwrap_or(Probes {
            hook: HookRead::Absent,
            pane: PaneRead::Absent,
        }))
    }

    fn first_run_prompt(&self, tail: &str) -> Option<PromptMatch> {
        let st = self.st();
        st.prompts
            .iter()
            .find(|(t, _)| tail.contains(t.as_str()))
            .map(|(t, p)| PromptMatch {
                prompt: *p,
                line: t.clone(),
            })
    }

    fn launch(&self, req: &LaunchReq) -> Result<SessionRef, EngineError> {
        let mut st = self.st();
        st.launches.push(req.clone());
        st.launch_results.pop_front().unwrap_or_else(|| {
            Ok(SessionRef {
                id: String::new(),
                handle: req.handle.clone(),
                workdir: req.workdir.clone(),
                worktree: req.worktree.clone(),
                pane: Some(PaneRef::new("p-1")),
                sandbox: None,
            })
        })
    }

    fn steer(
        &self,
        s: &SessionRef,
        steer: &Steer,
        _deadline: Deadline,
    ) -> Result<SteerOutcome, EngineError> {
        let mut st = self.st();
        st.steers.push((s.id.clone(), steer.clone()));
        st.steer_results.pop_front().unwrap_or(Ok(SteerOutcome {
            submit_retried: false,
            submit_checked: true,
        }))
    }

    fn transcript(&self, s: &SessionRef, dest: &Path) -> Result<(), EngineError> {
        let (hook, fail, text) = {
            let mut st = self.st();
            st.transcripts.push(s.id.clone());
            (
                st.on_transcript.clone(),
                st.fail_transcript.take(),
                st.later_texts
                    .pop_front()
                    .or_else(|| st.transcript_text.clone()),
            )
        };
        if let Some(f) = hook {
            f();
        }
        if let Some(e) = fail {
            return Err(e);
        }
        text.map_or(Ok(()), |t| {
            std::fs::write(dest, t).map_err(|e| EngineError::Transcript(e.to_string()))
        })
    }
}

#[cfg(test)]
mod tests;
