//! In-memory `Surface` for unit tests of ops.
#![allow(
    dead_code,
    clippy::significant_drop_tightening,
    reason = "test fake; not yet used by ops"
)]

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::activity::PaneObs;
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{
    Key, PaneCmd, PaneRef, ResumeOutcome, ResumeReg, Surface, SurfaceError, WorkspaceRef, Worktree,
};
use moor_core::TaskName;

#[derive(Default)]
struct State {
    n: u32,
    worktrees: Vec<PathBuf>,
    workspaces: Vec<String>,
    panes: Vec<String>,
    calls: Vec<String>,
    obs: HashMap<String, VecDeque<Result<PaneObs, SurfaceError>>>,
    opens: Vec<PaneCmd>,
    fail_ensure: Option<SurfaceError>,
    fail_open_pane: Option<SurfaceError>,
    fail_close_pane: Option<SurfaceError>,
    fail_close_ws: Option<SurfaceError>,
    fail_focus_ws: Option<SurfaceError>,
    resume_reply: Option<ResumeOutcome>,
}

/// Scripted Surface: worktrees at `<root>/wt/<name>`, workspaces `ws-N`, panes `p-N`.
pub(crate) struct FakeSurface {
    root: PathBuf,
    state: Mutex<State>,
}

impl FakeSurface {
    /// A Surface whose worktrees live under `root`; nothing touches disk.
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            state: Mutex::new(State::default()),
        }
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The next `ensure_workspace` fails once with `e`.
    pub(crate) fn fail_ensure_workspace(self, e: SurfaceError) -> Self {
        self.st().fail_ensure = Some(e);
        self
    }

    /// The next `open_pane` fails once with `e`.
    pub(crate) fn fail_open_pane(self, e: SurfaceError) -> Self {
        self.st().fail_open_pane = Some(e);
        self
    }

    /// Every command `open_pane` was given, in call order.
    pub(crate) fn opened(&self) -> Vec<PaneCmd> {
        self.st().opens.clone()
    }

    /// The next `close_pane` fails once with `e`.
    pub(crate) fn fail_close_pane(self, e: SurfaceError) -> Self {
        self.st().fail_close_pane = Some(e);
        self
    }

    /// The next `close_workspace` fails once with `e`.
    pub(crate) fn fail_close_workspace(self, e: SurfaceError) -> Self {
        self.st().fail_close_ws = Some(e);
        self
    }

    /// The next `focus_workspace` fails once with `e`.
    pub(crate) fn fail_focus_workspace(self, e: SurfaceError) -> Self {
        self.st().fail_focus_ws = Some(e);
        self
    }

    /// Every `register_resume` answers `outcome` instead of `Registered`.
    pub(crate) fn answer_resume(self, outcome: ResumeOutcome) -> Self {
        self.st().resume_reply = Some(outcome);
        self
    }

    /// Scripts the next `pane_obs` answer for `pane`; the last answer repeats.
    pub(crate) fn script_pane_obs(self, pane: &str, obs: Result<PaneObs, SurfaceError>) -> Self {
        self.push_pane_obs(pane, obs);
        self
    }

    /// [`Self::script_pane_obs`] for a surface already shared.
    pub(crate) fn push_pane_obs(&self, pane: &str, obs: Result<PaneObs, SurfaceError>) {
        self.st()
            .obs
            .entry(pane.to_owned())
            .or_default()
            .push_back(obs);
    }

    /// Calls so far, like `create_worktree a` or `close_pane p-1`.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.st().calls.clone()
    }
}

impl Surface for FakeSurface {
    fn create_worktree(
        &self,
        _repo: &Path,
        name: &TaskName,
        _base: Option<&str>,
    ) -> Result<Worktree, SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!("create_worktree {}", name.as_str()));
        let path = self.root.join("wt").join(name.as_str());
        s.worktrees.push(path.clone());
        Ok(Worktree {
            path,
            branch: name.as_str().to_owned(),
        })
    }

    fn remove_worktree(&self, wt: &Path) -> Result<(), SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!("remove_worktree {}", wt.display()));
        let before = s.worktrees.len();
        s.worktrees.retain(|p| p != wt);
        if s.worktrees.len() < before {
            Ok(())
        } else {
            Err(SurfaceError::NotFound)
        }
    }

    fn ensure_workspace(&self, wt: &Worktree, label: &str) -> Result<WorkspaceRef, SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!("ensure_workspace {label}"));
        if let Some(e) = s.fail_ensure.take() {
            return Err(e);
        }
        if !s.worktrees.contains(&wt.path) {
            return Err(SurfaceError::NotFound);
        }
        s.n += 1;
        let id = format!("ws-{}", s.n);
        s.workspaces.push(id.clone());
        Ok(WorkspaceRef::new(id))
    }

    fn open_pane(&self, ws: &WorkspaceRef, cmd: &PaneCmd) -> Result<PaneRef, SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!("open_pane {}", ws.as_str()));
        s.opens.push(cmd.clone());
        if let Some(e) = s.fail_open_pane.take() {
            return Err(e);
        }
        if !s.workspaces.iter().any(|w| w == ws.as_str()) {
            return Err(SurfaceError::NotFound);
        }
        s.n += 1;
        let id = format!("p-{}", s.n);
        s.panes.push(id.clone());
        Ok(PaneRef::new(id))
    }

    fn pane_obs(&self, pane: &PaneRef, _deadline: Deadline) -> Result<PaneObs, SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!("pane_obs {}", pane.as_str()));
        let queue = s.obs.get_mut(pane.as_str()).ok_or(SurfaceError::NotFound)?;
        if queue.len() > 1 {
            queue.pop_front().ok_or(SurfaceError::NotFound)?
        } else {
            queue.front().cloned().ok_or(SurfaceError::NotFound)?
        }
    }

    fn send_text(&self, pane: &PaneRef, text: &str) -> Result<(), SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!("send_text {} {text}", pane.as_str()));
        if s.panes.iter().any(|p| p == pane.as_str()) {
            Ok(())
        } else {
            Err(SurfaceError::NotFound)
        }
    }

    fn send_keys(&self, pane: &PaneRef, keys: &[Key]) -> Result<(), SurfaceError> {
        let mut s = self.st();
        let names: Vec<String> = keys.iter().filter_map(|k| k.name()).collect();
        s.calls
            .push(format!("send_keys {} {}", pane.as_str(), names.join(" ")));
        if s.panes.iter().any(|p| p == pane.as_str()) {
            Ok(())
        } else {
            Err(SurfaceError::NotFound)
        }
    }

    fn register_resume(
        &self,
        pane: &PaneRef,
        reg: &ResumeReg,
    ) -> Result<ResumeOutcome, SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!(
            "register_resume {} {} {}",
            pane.as_str(),
            reg.agent,
            reg.session_id
        ));
        if s.panes.iter().any(|p| p == pane.as_str()) {
            Ok(s.resume_reply.clone().unwrap_or(ResumeOutcome::Registered))
        } else {
            Err(SurfaceError::NotFound)
        }
    }

    fn close_pane(&self, pane: &PaneRef) -> Result<(), SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!("close_pane {}", pane.as_str()));
        if let Some(e) = s.fail_close_pane.take() {
            return Err(e);
        }
        let before = s.panes.len();
        s.panes.retain(|p| p != pane.as_str());
        if s.panes.len() < before {
            Ok(())
        } else {
            Err(SurfaceError::NotFound)
        }
    }

    fn focus_workspace(&self, ws: &WorkspaceRef) -> Result<(), SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!("focus_workspace {}", ws.as_str()));
        if let Some(e) = s.fail_focus_ws.take() {
            return Err(e);
        }
        Ok(())
    }

    fn close_workspace(&self, ws: &WorkspaceRef) -> Result<(), SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!("close_workspace {}", ws.as_str()));
        if let Some(e) = s.fail_close_ws.take() {
            return Err(e);
        }
        let before = s.workspaces.len();
        s.workspaces.retain(|w| w != ws.as_str());
        if s.workspaces.len() < before {
            Ok(())
        } else {
            Err(SurfaceError::NotFound)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use moor_core::activity::PaneStatus;

    use super::*;

    fn obs(tail: &str) -> PaneObs {
        PaneObs {
            status: PaneStatus::Idle,
            tail: tail.to_owned(),
            prompt: None,
            exit_marker: None,
        }
    }

    #[test]
    fn serves_scripted_observations_in_order_and_records_calls() {
        let s = FakeSurface::new("/r")
            .script_pane_obs("p-1", Ok(obs("a")))
            .script_pane_obs("p-1", Err(SurfaceError::Timeout));
        let p = PaneRef::new("p-1");
        let d = Deadline(Duration::from_secs(1));
        assert_eq!(s.pane_obs(&p, d), Ok(obs("a")));
        assert_eq!(s.pane_obs(&p, d), Err(SurfaceError::Timeout));
        assert_eq!(s.pane_obs(&p, d), Err(SurfaceError::Timeout));
        assert_eq!(
            s.pane_obs(&PaneRef::new("p-2"), d),
            Err(SurfaceError::NotFound)
        );
        assert_eq!(s.calls().len(), 4);
        assert_eq!(s.calls().first().map(String::as_str), Some("pane_obs p-1"));
    }

    #[test]
    fn records_sends_and_reports_a_missing_pane() {
        let s = FakeSurface::new("/r");
        let wt = s
            .create_worktree(Path::new("/repo"), &TaskName::parse("a").unwrap(), None)
            .unwrap();
        let ws = s.ensure_workspace(&wt, "moor:a").unwrap();
        let pane = s.open_pane(&ws, &PaneCmd::default()).unwrap();
        assert_eq!(s.send_text(&pane, "hi"), Ok(()));
        assert_eq!(s.send_keys(&pane, &[Key::CtrlU, Key::Digit(3)]), Ok(()));
        let gone = PaneRef::new("p-9");
        assert_eq!(s.send_text(&gone, "x"), Err(SurfaceError::NotFound));
        assert_eq!(
            s.send_keys(&gone, &[Key::CtrlM]),
            Err(SurfaceError::NotFound)
        );
        let sent: Vec<String> = s.calls().into_iter().skip(3).collect();
        assert_eq!(
            sent,
            [
                "send_text p-2 hi",
                "send_keys p-2 ctrl+u 3",
                "send_text p-9 x",
                "send_keys p-9 ctrl+m"
            ]
        );
    }
}
