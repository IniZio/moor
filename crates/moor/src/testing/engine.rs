//! In-memory `Engine` for unit tests of ops.
#![allow(
    dead_code,
    clippy::significant_drop_tightening,
    reason = "test fake; not yet used by ops"
)]

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use moor_core::activity::{HookKind, HookRead, HookRecord, PaneRead, Probes};
use moor_core::name::BoxName;
use moor_core::name::BoxPath;
use moor_core::seam::agent_adapter::{AgentInstall, FirstRunPrompt, PromptMatch};
use moor_core::seam::clock::Clock;
use moor_core::seam::engine::{
    AgentFate, Engine, EngineError, LaunchReq, SessionRef, Steer, SteerOutcome,
};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::PaneRef;
use moor_core::seam::surface::WorkspaceRef;

#[derive(Default)]
struct State {
    probes: HashMap<String, Probes>,
    queued: HashMap<String, VecDeque<Probes>>,
    fail_probe: HashMap<String, EngineError>,
    counts: HashMap<String, usize>,
    panes: HashMap<String, Option<PaneRef>>,
    launch_results: VecDeque<Result<SessionRef, EngineError>>,
    launches: Vec<LaunchReq>,
    box_home: Option<String>,
    fail_launch_dirs: Option<String>,
    prompts: Vec<(String, FirstRunPrompt)>,
    quiet_first_run: bool,
    steer_results: VecDeque<Result<SteerOutcome, EngineError>>,
    steers: Vec<(String, Steer)>,
    transcripts: Vec<String>,
    transcript_text: Option<String>,
    later_texts: VecDeque<String>,
    fail_transcript: Option<EngineError>,
    on_transcript: Option<Arc<dyn Fn() + Send + Sync>>,
    hang: Option<Arc<dyn Clock>>,
    install: Option<AgentInstall>,
    fates: VecDeque<Result<AgentFate, EngineError>>,
    reattached: Vec<(String, String)>,
    /// Probes a session reads from once a steer is sent to it.
    after_steer: Vec<(String, Probes)>,
}

/// The launch a scripted starting marker names to stand for the latest launch's own marker.
pub(crate) const THIS_LAUNCH: &str = "this-launch";

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

    /// Every probe hangs until its deadline passes on `clock`, then reads nothing.
    pub(crate) fn hangs_on(self, clock: Arc<dyn Clock>) -> Self {
        self.st().hang = Some(clock);
        self
    }

    /// The Engine knows how to install its agent with `install`.
    pub(crate) fn install(self, install: AgentInstall) -> Self {
        self.st().install = Some(install);
        self
    }

    /// Launches create their directories under `home`; unset, the launch creates none.
    pub(crate) fn box_home(self, home: &str) -> Self {
        self.st().box_home = Some(home.to_owned());
        self
    }

    /// Every later `launch_dirs` fails with `message`.
    pub(crate) fn fail_launch_dirs(self, message: &str) -> Self {
        self.st().fail_launch_dirs = Some(message.to_owned());
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

    /// The agent runs no hook while a first-run screen is up.
    pub(crate) fn hooks_quiet_during_first_run(self) -> Self {
        self.st().quiet_first_run = true;
        self
    }

    /// The next steer returns `r`; steers past the scripted ones succeed without a retry.
    pub(crate) fn steer_result(self, r: Result<SteerOutcome, EngineError>) -> Self {
        self.st().steer_results.push_back(r);
        self
    }

    /// From now on `session` probes as `p`.
    pub(crate) fn set_probes(&self, session: &str, p: Probes) {
        self.st().probes.insert(session.to_owned(), p);
    }

    /// Once a steer reaches `session`, its probes read `p`: what the agent wrote while the
    /// steer was still being sent.
    pub(crate) fn probes_after_steer(self, session: &str, p: Probes) -> Self {
        self.st().after_steer.push((session.to_owned(), p));
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
    /// Queues what the next calls to `agent_fate` answer, in order; an empty queue answers
    /// that the agent ended with its pane.
    pub(crate) fn fates(self, fates: Vec<Result<AgentFate, EngineError>>) -> Self {
        self.st().fates.extend(fates);
        self
    }

    /// Every reattach so far, as (session id, new pane).
    pub(crate) fn reattached(&self) -> Vec<(String, String)> {
        self.st().reattached.clone()
    }

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
    fn probe(&self, s: &SessionRef, deadline: Deadline) -> Result<Probes, EngineError> {
        let mut st = self.st();
        *st.counts.entry(s.id.clone()).or_default() += 1;
        st.panes.insert(s.id.clone(), s.pane.clone());
        if let Some(clock) = st.hang.clone() {
            drop(st);
            clock.sleep(deadline.0);
            return Ok(Probes {
                hook: HookRead::TimedOut,
                pane: PaneRead::TimedOut,
            });
        }
        if let Some(e) = st.fail_probe.remove(&s.id) {
            return Err(e);
        }
        let mut p = st
            .queued
            .get_mut(&s.id)
            .and_then(VecDeque::pop_front)
            .or_else(|| st.probes.get(&s.id).cloned())
            .unwrap_or(Probes {
                hook: HookRead::Absent,
                pane: PaneRead::Absent,
            });
        if let HookRead::Fresh(HookRecord {
            kind: HookKind::Starting { launch },
            ..
        }) = &mut p.hook
        {
            if launch.as_deref() == Some(THIS_LAUNCH) {
                *launch = st.launches.last().map(|l| l.nonce.clone());
            }
        }
        Ok(p)
    }

    fn hooks_quiet_during_first_run(&self) -> bool {
        self.st().quiet_first_run
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

    fn agent_install(&self) -> Option<AgentInstall> {
        self.st().install.clone()
    }

    fn launch(&self, req: &LaunchReq) -> Result<SessionRef, EngineError> {
        let mut st = self.st();
        st.launches.push(req.clone());
        st.launch_results.pop_front().unwrap_or_else(|| {
            Ok(SessionRef {
                id: String::new(),
                handle: req.handle.clone(),
                workdir: req.workdir.clone(),
                pane: Some(PaneRef::new("p-1")),
                warnings: Vec::new(),
            })
        })
    }

    fn launch_dirs(&self, req: &LaunchReq) -> Result<Option<(BoxPath, BoxPath)>, EngineError> {
        let (home, failed) = {
            let st = self.st();
            (st.box_home.clone(), st.fail_launch_dirs.clone())
        };
        let err = |message: String| EngineError::Failed {
            step: "home",
            message,
        };
        if let Some(message) = failed {
            return Err(err(message));
        }
        let Some(home) = home else { return Ok(None) };
        let home = BoxPath::restore(&home).map_err(|e| err(e.to_string()))?;
        req.dirs_under(&home)
            .map(Some)
            .map_err(|e| err(e.to_string()))
    }

    fn steer(
        &self,
        s: &SessionRef,
        steer: &Steer,
        _deadline: Deadline,
    ) -> Result<SteerOutcome, EngineError> {
        let mut st = self.st();
        st.steers.push((s.id.clone(), steer.clone()));
        if let Some(i) = st.after_steer.iter().position(|(id, _)| *id == s.id) {
            let (id, p) = st.after_steer.remove(i);
            st.probes.insert(id, p);
        }
        st.steer_results.pop_front().unwrap_or(Ok(SteerOutcome {
            submit_retried: false,
            submit_checked: true,
        }))
    }

    fn agent_fate(&self, _s: &SessionRef, _deadline: Deadline) -> Result<AgentFate, EngineError> {
        self.st()
            .fates
            .pop_front()
            .unwrap_or(Ok(AgentFate::Ended { exit: None }))
    }

    fn reattach(
        &self,
        s: &SessionRef,
        _ws: &WorkspaceRef,
        _name: &BoxName,
    ) -> Result<PaneRef, EngineError> {
        let mut st = self.st();
        let pane = format!("reattached-{}", st.reattached.len() + 1);
        st.reattached.push((s.id.clone(), pane.clone()));
        Ok(PaneRef::new(pane))
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
