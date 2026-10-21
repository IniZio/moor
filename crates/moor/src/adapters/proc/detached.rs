//! Detached daemon spawn: own process group, stderr appended to a log file.

use std::ffi::OsString;
use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

use moor_core::seam::proc::{ProcError, ProcSpec, Tool};

use super::SystemProc;

/// Mode of a created log directory: owner only.
const DIR_MODE: u32 = 0o700;
/// Mode of a created log file: owner only.
const FILE_MODE: u32 = 0o600;

impl SystemProc {
    /// Starts `spec` in its own process group with stdin and stdout null and
    /// stderr appended to `log`, so it outlives this process and the terminal.
    /// Returns the child's pid; a background thread reaps it.
    pub(super) fn daemon(&self, spec: &ProcSpec, log: &Path) -> Result<u32, ProcError> {
        let mut cmd = self.daemon_command(spec);
        if let Some(dir) = log.parent() {
            DirBuilder::new()
                .recursive(true)
                .mode(DIR_MODE)
                .create(dir)
                .map_err(ProcError::Spawn)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(FILE_MODE)
            .open(log)
            .map_err(ProcError::Spawn)?;
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(file));
        let mut child = cmd.spawn().map_err(ProcError::Spawn)?;
        let pid = child.id();
        thread::spawn(move || drop(child.wait()));
        Ok(pid)
    }

    /// Like `command`, but moor itself resolves to the running executable
    /// unless an override or config path names another program.
    ///
    /// The proxy supervisor is moor itself, so it keeps this process's
    /// environment: it must resolve the same state dir, config and credential
    /// sources as the moor that launched it. Other tools get a cleared one.
    fn daemon_command(&self, spec: &ProcSpec) -> Command {
        if spec.tool != Tool::MoorSelf {
            return self.command(spec);
        }
        let program = self.program(spec.tool);
        let program = if program == Tool::MoorSelf.program() {
            std::env::current_exe().map_or(program, OsString::from)
        } else {
            program
        };
        let mut cmd = Command::new(program);
        cmd.args(&spec.args).process_group(0);
        cmd.envs(spec.env.extra.iter().map(|(k, v)| (k, v)));
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        cmd
    }
}

#[cfg(test)]
mod tests;
