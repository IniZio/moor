//! Surface adapter over herdr on a dedicated session.

mod errors;
mod pane;
mod parse;
mod resume;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use moor_core::activity::PaneObs;
use moor_core::seam::clock::Clock;
use moor_core::seam::proc::{Deadline, Proc, ProcSpec, Tool};
use moor_core::seam::surface::{
    Key, PaneCmd, PaneRef, ResumeOutcome, ResumeReg, Surface, SurfaceError, WorkspaceRef,
};

use self::errors::{map_error, HerdrFailure};

/// Ceiling for one herdr call.
const CALL_DEADLINE: Duration = Duration::from_secs(60);
/// Pauses between retries while a freshly started server comes up.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(150),
    Duration::from_millis(400),
    Duration::from_millis(1000),
];

/// Surface backed by herdr.
pub(crate) struct Herdr {
    /// Process spawner.
    proc: Arc<dyn Proc>,
    /// Dedicated herdr session name.
    session: String,
    /// Host variables herdr needs, added to every call.
    extra_env: Vec<(String, String)>,
    /// Last sequence number sent with a report.
    seq: AtomicU64,
    /// Where sequence numbers start from, when set.
    clock: Option<Arc<dyn Clock>>,
    /// Working directory of new Workspaces; herdr's own when unset.
    workspace_dir: Option<PathBuf>,
}

/// Quotes `s` for a POSIX shell when it needs it.
fn shell_word(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if plain {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

impl Herdr {
    /// Creates the adapter for `session`.
    pub(crate) fn new(
        proc: Arc<dyn Proc>,
        session: String,
        extra_env: Vec<(String, String)>,
    ) -> Self {
        Self {
            proc,
            session,
            extra_env,
            seq: AtomicU64::new(0),
            clock: None,
            workspace_dir: None,
        }
    }

    /// Opens new Workspaces in `dir`.
    pub(crate) fn with_workspace_dir(mut self, dir: PathBuf) -> Self {
        self.workspace_dir = Some(dir);
        self
    }

    /// Builds a herdr spec carrying the session and host env.
    fn spec(&self, args: &[&str], deadline: Deadline) -> ProcSpec {
        let spec = ProcSpec::new(Tool::Herdr, deadline)
            .args(args.iter().copied())
            .env("HERDR_SESSION", &self.session);
        self.extra_env
            .iter()
            .fold(spec, |s, (k, v)| s.env(k.as_str(), v.as_str()))
    }

    /// Runs one call, mapping failures through the error table.
    fn run_once(&self, args: &[&str]) -> Result<String, HerdrFailure> {
        self.run_within(args, Deadline(CALL_DEADLINE))
    }

    /// Runs one call under `deadline`, mapping failures through the error table.
    fn run_within(&self, args: &[&str], deadline: Deadline) -> Result<String, HerdrFailure> {
        self.proc
            .run(&self.spec(args, deadline))
            .map(|o| o.stdout)
            .map_err(|e| map_error(&e))
    }

    /// Runs a call; starts the session server and retries when it is down.
    fn call(&self, args: &[&str]) -> Result<String, SurfaceError> {
        match self.run_once(args) {
            Ok(out) => Ok(out),
            Err(HerdrFailure::Surface(e)) => Err(e),
            Err(HerdrFailure::ServerNotRunning) => self.start_and_retry(args),
        }
    }

    /// Starts the headless server, then retries `args` while it is down.
    fn start_and_retry(&self, args: &[&str]) -> Result<String, SurfaceError> {
        self.proc
            .spawn_detached(&self.spec(&["server"], Deadline(CALL_DEADLINE)))
            .map_err(|e| SurfaceError::Unavailable(e.to_string()))?;
        for delay in RETRY_DELAYS {
            sleep(delay);
            match self.run_once(args) {
                Ok(out) => return Ok(out),
                Err(HerdrFailure::Surface(e)) => return Err(e),
                Err(HerdrFailure::ServerNotRunning) => {}
            }
        }
        Err(SurfaceError::Unavailable(
            "herdr server did not start".to_owned(),
        ))
    }

    /// Gives `ws` the label `label`.
    fn rename(&self, ws: &str, label: &str) -> Result<(), SurfaceError> {
        self.call(&["workspace", "rename", ws, label]).map(drop)
    }
}

impl Surface for Herdr {
    fn ensure_workspace(&self, label: &str) -> Result<WorkspaceRef, SurfaceError> {
        let listed = parse::workspace_list(&self.call(&["workspace", "list"])?)?;
        if let Some(found) = listed
            .into_iter()
            .find(|w| w.label.as_deref() == Some(label))
        {
            return Ok(WorkspaceRef::new(found.workspace_id));
        }
        let dir = self.workspace_dir.as_deref().map(Path::to_string_lossy);
        let mut args = vec!["workspace", "create"];
        if let Some(dir) = &dir {
            args.extend(["--cwd", dir]);
        }
        args.push("--no-focus");
        let id = parse::workspace_created(&self.call(&args)?)?;
        self.rename(&id, label)?;
        Ok(WorkspaceRef::new(id))
    }

    fn open_pane(&self, ws: &WorkspaceRef, cmd: &PaneCmd) -> Result<PaneRef, SurfaceError> {
        if cmd.argv.is_empty() {
            return Err(SurfaceError::Failed {
                summary: "open pane: empty command".to_owned(),
            });
        }
        let envs: Vec<String> = cmd.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let mut args = vec![
            "tab",
            "create",
            "--workspace",
            ws.as_str(),
            "--label",
            &cmd.label,
        ];
        for e in &envs {
            args.extend(["--env", e]);
        }
        args.push("--no-focus");
        let pane = parse::tab_created(&self.call(&args)?)?;
        let line = cmd
            .argv
            .iter()
            .map(|a| shell_word(a))
            .collect::<Vec<_>>()
            .join(" ");
        if let Err(e) = self.call(&["pane", "run", &pane, &line]) {
            // The empty tab would stay open; the run error is the one to report.
            _ = self.call(&["pane", "close", &pane]);
            return Err(e);
        }
        Ok(PaneRef::new(pane))
    }

    fn send_text(&self, pane: &PaneRef, text: &str) -> Result<(), SurfaceError> {
        self.call(&["pane", "send-text", pane.as_str(), text])
            .map(drop)
    }

    fn send_keys(&self, pane: &PaneRef, keys: &[Key]) -> Result<(), SurfaceError> {
        let names = keys
            .iter()
            .map(|k| {
                k.name().ok_or_else(|| SurfaceError::Failed {
                    summary: format!("send keys: unknown key {k:?}"),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut args = vec!["pane", "send-keys", pane.as_str()];
        args.extend(names.iter().map(String::as_str));
        self.call(&args).map(drop)
    }

    fn register_resume(
        &self,
        pane: &PaneRef,
        reg: &ResumeReg,
    ) -> Result<ResumeOutcome, SurfaceError> {
        self.register(pane, reg)
    }

    fn close_pane(&self, pane: &PaneRef) -> Result<(), SurfaceError> {
        self.call(&["pane", "close", pane.as_str()]).map(drop)
    }

    fn pane_obs(&self, pane: &PaneRef, deadline: Deadline) -> Result<PaneObs, SurfaceError> {
        self.observe(pane, deadline)
    }

    fn focus_workspace(&self, ws: &WorkspaceRef) -> Result<(), SurfaceError> {
        self.call(&["workspace", "focus", ws.as_str()]).map(drop)
    }

    fn close_workspace(&self, ws: &WorkspaceRef) -> Result<(), SurfaceError> {
        self.call(&["workspace", "close", ws.as_str()]).map(drop)
    }
}
