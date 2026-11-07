//! Real work trees for the Driver fake: each Box is a directory, its commands run there under
//! `/bin/sh`, a checkpoint is a copy of it and a fork is a copy of that. So a test sees what a Box's
//! disk really holds after moor's scripts ran with real git, without a VM.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::MutexGuard;

use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Output;

use super::{FakeDriver, State};

/// Where the Boxes live and what stands for GitHub.
#[derive(Clone)]
pub(super) struct Trees {
    /// Holds `<handle>/{repo,home}` and `ckpt/<id>`.
    root: PathBuf,
    /// The URL that stands for `https://github.com/` in a Box's commands, ending in `/`.
    remote: String,
}

/// What a Box's repo URL starts with.
const GITHUB: &str = "https://github.com/";

fn failed(what: &str, e: impl std::fmt::Display) -> DriverError {
    DriverError::Failed {
        summary: format!("{what}: {e}"),
    }
}

/// Copies `from` to `to`, keeping modes and links.
fn copy(from: &Path, to: &Path) -> Result<(), DriverError> {
    let done = Command::new("cp").arg("-a").arg(from).arg(to).status();
    match done {
        Ok(s) if s.success() => Ok(()),
        other => Err(failed("cp", format!("{other:?}"))),
    }
}

impl Trees {
    pub(super) fn acquired(&self, handle: &str) -> Result<(), DriverError> {
        let dir = self.root.join(handle);
        std::fs::create_dir_all(dir.join("repo")).map_err(|e| failed("mkdir", e))?;
        moor_testkit::write_host_guards(&dir.join("home/bin")).map_err(|e| failed("guards", e))
    }

    pub(super) fn saved(&self, handle: &str, id: &str) -> Result<(), DriverError> {
        let dir = self.root.join("ckpt");
        std::fs::create_dir_all(&dir).map_err(|e| failed("mkdir", e))?;
        copy(&self.root.join(handle), &dir.join(id))
    }

    pub(super) fn forked(&self, id: &str, handle: &str) -> Result<(), DriverError> {
        copy(&self.root.join("ckpt").join(id), &self.root.join(handle))
    }

    /// Runs `spec` in the Box's tree, whatever path its disk was saved under. The prelude is left
    /// out, since it sets up the Proxy this host has none of.
    pub(super) fn exec(&self, handle: &str, spec: &ExecSpec) -> Result<ExecOutcome, DriverError> {
        let line = match spec.argv.as_slice() {
            [sh, c, _prelude, name, rest @ ..] if sh == "sh" && c == "-c" && name == "sh" => rest,
            all => all,
        };
        let (program, args) = line
            .split_first()
            .ok_or_else(|| failed("exec", "no argv"))?;
        let (stdout, stderr) = match &spec.output {
            Some(Output::Log(path)) => {
                let log = File::create(path).map_err(|e| failed("log", e))?;
                let again = log.try_clone().map_err(|e| failed("log", e))?;
                (Stdio::from(log), Stdio::from(again))
            }
            _ => (Stdio::null(), Stdio::null()),
        };
        let dir = self.root.join(handle);
        let status = Command::new(program)
            .args(args.iter().map(|a| a.replace(GITHUB, &self.remote)))
            .env_clear()
            .env("HOME", dir.join("home"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", dir.join("home/bin").display()),
            )
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .current_dir(dir.join("repo"))
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .status()
            .map_err(|e| failed("exec", e))?;
        Ok(ExecOutcome::Exited {
            code: status.code().unwrap_or(1),
        })
    }

    /// Copies the file at `rel` under the Box's tree to `to`.
    pub(super) fn copy_out(&self, handle: &str, rel: &str, to: &Path) -> Result<(), DriverError> {
        let from = self.root.join(handle).join("repo").join(rel);
        std::fs::copy(from, to).map_err(|e| failed("copy", e))?;
        Ok(())
    }
}

/// Runs `spec` in the real tree of the Box behind `handle`, unless it was set up to end otherwise,
/// outside the fake's lock.
pub(super) fn exec_in_tree(
    mut s: MutexGuard<'_, State>,
    handle: &str,
    spec: &ExecSpec,
) -> Result<ExecOutcome, DriverError> {
    let argv = spec.argv.join(" ");
    let ended = s.exec_ends.iter().find(|(n, _)| argv.contains(n.as_str()));
    if let Some(out) = ended.map(|(_, out)| *out).or_else(|| s.end_once(&argv)) {
        return Ok(out);
    }
    let trees = s.trees.clone();
    drop(s);
    trees.map_or(Err(DriverError::NotFound), |t| t.exec(handle, spec))
}

impl FakeDriver {
    /// Runs every Box in a real directory under `root`, with `remote` standing for
    /// `https://github.com/`.
    pub(crate) fn real_trees(self, root: &Path, remote: &str) -> Self {
        self.st().trees = Some(Trees {
            root: root.to_path_buf(),
            remote: remote.to_owned(),
        });
        self
    }

    /// The work tree of the Box behind `handle`, when Boxes are real.
    pub(crate) fn tree_of(&self, handle: &str) -> Option<PathBuf> {
        let trees = self.st().trees.clone()?;
        Some(trees.root.join(handle).join("repo"))
    }

    /// The home of the Box behind `handle`, when Boxes are real.
    pub(crate) fn home_of(&self, handle: &str) -> Option<PathBuf> {
        let trees = self.st().trees.clone()?;
        Some(trees.root.join(handle).join("home"))
    }
}
