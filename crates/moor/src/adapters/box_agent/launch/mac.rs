//! The launch of a Task whose Box is a macOS layout: the Box home is the Task's own, so the
//! launch reads no process environment and prepares the Task's directories instead.

use moor_core::boxscript::quote;
use moor_core::exec_env::MacTask;
use moor_core::name::BoxPath;
use moor_core::seam::driver::{BoxStep, ExecOutcome, StepDone};
use moor_core::seam::engine::LaunchReq;
use moor_core::task_key::{MacLayout, TaskKey};

use super::install::StepResult;
use super::{fail, Failure, Launch};

/// The macOS layout of a launch's Task, before the Box has been asked anything.
pub(super) struct Mac {
    /// Every directory of the Task's Box.
    layout: MacLayout,
    /// The Task's key.
    key: TaskKey,
}

/// A launch's Task on a macOS Box, with what the prepare step printed. The Box home is the
/// Task's own; `login_home` is the Box user's real home, used only for read grants outside the
/// Task.
pub(super) struct MacBox {
    /// Every directory of the Task's Box.
    pub(super) layout: MacLayout,
    /// The Box user's real login home, not the Task's.
    pub(super) login_home: BoxPath,
    /// The Xcode developer directory, as resolved once by the prepare step.
    pub(super) developer_dir: BoxPath,
    /// The Box user is root.
    pub(super) root: bool,
}

impl MacBox {
    /// The Box user's home: the Task's own.
    pub(super) const fn home(&self) -> &BoxPath {
        &self.layout.home
    }
}

impl Launch<'_> {
    /// The home that holds the Box user's own binaries (`nono`, `gh`, the agent command): the
    /// real login home on a macOS Box, whose Task home is the agent's alone, and the Box home
    /// elsewhere. The batch steps and the launch script start in the login home.
    pub(super) fn bin_home(&self) -> &BoxPath {
        self.mac.map_or(&self.ctx.home, |m| &m.login_home)
    }
}

impl Mac {
    /// The layout of `req`'s Task, when its workdir is in one.
    pub(super) fn find(req: &LaunchReq) -> Option<Self> {
        let key = TaskKey::new(&req.task);
        MacLayout::find(&req.workdir, &key).map(|layout| Self { layout, key })
    }

    /// The Box user's home: the Task's own.
    pub(super) const fn home(&self) -> &BoxPath {
        &self.layout.home
    }

    /// What the exec environment needs of the Task, for the launch with `nonce`.
    pub(super) const fn task<'a>(&'a self, nonce: &'a str) -> MacTask<'a> {
        MacTask {
            layout: &self.layout,
            key: &self.key,
            nonce,
        }
    }

    /// The step that prepares the Task's directories and launch files, then prints the user id,
    /// the login shell's home and the resolved developer directory, one per line.
    pub(super) fn prepare_step(&self) -> Result<BoxStep, Failure> {
        let script = self.layout.prepare_script().map_err(|e| fail("home", &e))?;
        let file = self
            .layout
            .developer_dir_file()
            .map_err(|e| fail("home", &e))?;
        Ok(BoxStep::Run(format!(
            "{script}\nid -u\nprintf '%s\\n' \"$HOME\"\ncat {}",
            quote(file.as_str())
        )))
    }

    /// The Task on its Box, from the result of the prepare step.
    pub(super) fn user(self, done: Option<StepResult>) -> Result<MacBox, Failure> {
        let stdout = match done {
            Some(Ok(StepDone::Ran {
                outcome: ExecOutcome::Exited { code: 0 },
                stdout,
            })) => stdout,
            Some(Ok(StepDone::Ran {
                outcome: ExecOutcome::Exited { code },
                ..
            })) => {
                return Err(fail(
                    "home",
                    &format!("could not prepare the Task directories: exit {code}"),
                ))
            }
            Some(Err(e)) => return Err(fail("home", &e)),
            _ => return Err(fail("home", &"the Task directories were not prepared")),
        };
        let mut lines = stdout.lines().map(str::trim);
        let root = lines.next() == Some("0");
        let path = |what: &str, line: Option<&str>| {
            BoxPath::restore(line.unwrap_or_default())
                .map_err(|e| fail("home", &format!("the Box user's {what} is not usable: {e}")))
        };
        let login_home = path("login home", lines.next())?;
        let developer_dir = path("developer directory", lines.next())?;
        Ok(MacBox {
            layout: self.layout,
            login_home,
            developer_dir,
            root,
        })
    }
}
