//! Surface adapter that runs each pane under a pseudo-terminal in this process.

mod screen;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use moor_core::activity::{PaneObs, PaneStatus};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{
    Key, PaneCmd, PaneRef, ResumeOutcome, ResumeReg, Surface, SurfaceError, WorkspaceRef,
};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

use self::screen::Msg;
use super::herdr::pane::exit_marker;

/// Terminal size every pane gets.
const SIZE: PtySize = PtySize {
    rows: 50,
    cols: 160,
    pixel_width: 0,
    pixel_height: 0,
};
/// Lines of pane text one observation returns.
const TAIL_LINES: usize = 60;
/// What a pane's terminal advertises.
const TERM: &str = "xterm-256color";

/// One running pane.
struct Pane {
    /// The Workspace that holds the pane.
    ws: WorkspaceRef,
    /// Requests to the thread that owns the pane's terminal.
    screen: mpsc::Sender<Msg>,
    /// The pty's input side, shared with the screen thread's replies.
    keys: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Kills the pane's process.
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Held so the pty stays open until the pane closes.
    _master: Box<dyn MasterPty + Send>,
}

/// Surface whose panes are pseudo-terminals owned by this process.
#[derive(Default)]
pub(crate) struct PtySurface {
    /// Workspace labels to their handles.
    workspaces: Mutex<BTreeMap<String, WorkspaceRef>>,
    /// Open panes by handle.
    panes: Mutex<HashMap<PaneRef, Pane>>,
    /// Last handle number given out.
    seq: AtomicU64,
    /// Working directory of new panes; this process's own when unset.
    cwd: Option<PathBuf>,
}

/// Locks `m`, taking the data a panicked holder left.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A failed pty call.
fn failed(what: &str, e: impl std::fmt::Display) -> SurfaceError {
    SurfaceError::Failed {
        summary: format!("{what}: {e}"),
    }
}

/// The bytes a terminal sends for `key`, `None` for a digit above 9.
const fn key_bytes(key: Key) -> Option<u8> {
    match key {
        Key::CtrlU => Some(0x15),
        Key::CtrlM => Some(b'\r'),
        Key::Escape => Some(0x1b),
        Key::Digit(d) if d <= 9 => Some(b'0' + d),
        Key::Digit(_) => None,
    }
}

impl PtySurface {
    /// Starts new panes in `cwd`.
    pub(crate) fn with_cwd(mut self, cwd: Option<PathBuf>) -> Self {
        self.cwd = cwd;
        self
    }

    /// A fresh handle text starting with `prefix`.
    fn next_id(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// Writes `bytes` to `pane`'s input.
    fn write(&self, pane: &PaneRef, bytes: &[u8]) -> Result<(), SurfaceError> {
        let keys = Arc::clone(
            &lock(&self.panes)
                .get(pane)
                .ok_or(SurfaceError::NotFound)?
                .keys,
        );
        let mut keys = lock(&keys);
        keys.write_all(bytes)
            .and_then(|()| keys.flush())
            .map_err(|e| failed("write to pane", e))
    }

    /// Starts `cmd` under a new pty.
    fn spawn(&self, ws: &WorkspaceRef, cmd: &PaneCmd) -> Result<Pane, SurfaceError> {
        let pair = native_pty_system()
            .openpty(SIZE)
            .map_err(|e| failed("open pty", e))?;
        let mut builder = CommandBuilder::from_argv(cmd.argv.iter().map(Into::into).collect());
        builder.env("TERM", TERM);
        for (k, v) in &cmd.env {
            builder.env(k, v);
        }
        match &self.cwd {
            Some(dir) => builder.cwd(dir),
            None => builder.cwd(std::env::current_dir().map_err(|e| failed("pane cwd", e))?),
        }
        let mut child = pair
            .slave
            .spawn_command(builder)
            .map_err(|e| failed("start pane", e))?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let ends = pair
            .master
            .try_clone_reader()
            .and_then(|r| Ok((r, pair.master.take_writer()?)));
        let (reader, writer) = match ends {
            Ok(ends) => ends,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed("open pty", e));
            }
        };
        thread::spawn(move || {
            let _ = child.wait();
        });
        let keys = Arc::new(Mutex::new(writer));
        let screen = screen::start(reader, Arc::clone(&keys), SIZE);
        Ok(Pane {
            ws: ws.clone(),
            screen,
            keys,
            killer,
            _master: pair.master,
        })
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}

impl Surface for PtySurface {
    fn ensure_workspace(&self, label: &str) -> Result<WorkspaceRef, SurfaceError> {
        let mut workspaces = lock(&self.workspaces);
        if let Some(ws) = workspaces.get(label) {
            return Ok(ws.clone());
        }
        let ws = WorkspaceRef::new(self.next_id("pty-ws"));
        workspaces.insert(label.to_owned(), ws.clone());
        drop(workspaces);
        Ok(ws)
    }

    fn focus_workspace(&self, _ws: &WorkspaceRef) -> Result<(), SurfaceError> {
        Ok(())
    }

    fn open_pane(&self, ws: &WorkspaceRef, cmd: &PaneCmd) -> Result<PaneRef, SurfaceError> {
        if cmd.argv.is_empty() {
            return Err(SurfaceError::Failed {
                summary: "open pane: empty command".to_owned(),
            });
        }
        let pane = self.spawn(ws, cmd)?;
        let id = PaneRef::new(self.next_id("pty-p"));
        lock(&self.panes).insert(id.clone(), pane);
        Ok(id)
    }

    fn pane_obs(&self, pane: &PaneRef, deadline: Deadline) -> Result<PaneObs, SurfaceError> {
        let screen = lock(&self.panes)
            .get(pane)
            .ok_or(SurfaceError::NotFound)?
            .screen
            .clone();
        let (reply, answer) = mpsc::channel();
        screen
            .send(Msg::Tail(TAIL_LINES, reply))
            .map_err(|_| SurfaceError::Unavailable("pane terminal ended".to_owned()))?;
        let text = answer.recv_timeout(deadline.0).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => SurfaceError::Timeout,
            mpsc::RecvTimeoutError::Disconnected => {
                SurfaceError::Unavailable("pane terminal ended".to_owned())
            }
        })?;
        Ok(PaneObs {
            status: PaneStatus::Unknown,
            prompt: None,
            exit_marker: exit_marker(&text),
            tail: text,
        })
    }

    fn send_text(&self, pane: &PaneRef, text: &str) -> Result<(), SurfaceError> {
        self.write(pane, text.as_bytes())
    }

    fn send_keys(&self, pane: &PaneRef, keys: &[Key]) -> Result<(), SurfaceError> {
        let bytes = keys
            .iter()
            .map(|k| {
                key_bytes(*k).ok_or_else(|| SurfaceError::Failed {
                    summary: format!("send keys: unknown key {k:?}"),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.write(pane, &bytes)
    }

    /// The agent outlives this process in tmux on the Box, and the next `open_pane`
    /// attaches to it again, so there is nothing to register.
    fn register_resume(
        &self,
        pane: &PaneRef,
        _reg: &ResumeReg,
    ) -> Result<ResumeOutcome, SurfaceError> {
        if lock(&self.panes).contains_key(pane) {
            Ok(ResumeOutcome::Registered)
        } else {
            Err(SurfaceError::NotFound)
        }
    }

    fn close_pane(&self, pane: &PaneRef) -> Result<(), SurfaceError> {
        let closed = lock(&self.panes).remove(pane);
        closed.map(drop).ok_or(SurfaceError::NotFound)
    }

    fn close_workspace(&self, ws: &WorkspaceRef) -> Result<(), SurfaceError> {
        let mut workspaces = lock(&self.workspaces);
        let before = workspaces.len();
        workspaces.retain(|_, w| w != ws);
        if workspaces.len() == before {
            return Err(SurfaceError::NotFound);
        }
        drop(workspaces);
        let mut panes = lock(&self.panes);
        let ids: Vec<PaneRef> = panes
            .iter()
            .filter(|(_, p)| &p.ws == ws)
            .map(|(id, _)| id.clone())
            .collect();
        let closed: Vec<Pane> = ids.iter().filter_map(|id| panes.remove(id)).collect();
        drop(panes);
        drop(closed);
        Ok(())
    }
}
