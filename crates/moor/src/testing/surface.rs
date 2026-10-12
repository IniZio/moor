//! In-memory `Surface` for unit tests of ops.
#![allow(
    dead_code,
    clippy::significant_drop_tightening,
    reason = "test fake; consumers land with the ops slices"
)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::seam::surface::{PaneCmd, PaneRef, Surface, SurfaceError, WorkspaceRef, Worktree};
use moor_core::TaskName;

#[derive(Default)]
struct State {
    n: u32,
    worktrees: Vec<PathBuf>,
    workspaces: Vec<String>,
    panes: Vec<String>,
    calls: Vec<String>,
    fail_ensure: Option<SurfaceError>,
    fail_close_pane: Option<SurfaceError>,
    fail_close_ws: Option<SurfaceError>,
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

    fn open_pane(&self, ws: &WorkspaceRef, _cmd: &PaneCmd) -> Result<PaneRef, SurfaceError> {
        let mut s = self.st();
        s.calls.push(format!("open_pane {}", ws.as_str()));
        if !s.workspaces.iter().any(|w| w == ws.as_str()) {
            return Err(SurfaceError::NotFound);
        }
        s.n += 1;
        let id = format!("p-{}", s.n);
        s.panes.push(id.clone());
        Ok(PaneRef::new(id))
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
