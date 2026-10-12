//! Credential sources: check and read without ever echoing a value.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::CredentialSource;
use moor_core::seam::proc::{Deadline, Proc, ProcError, ProcSpec, Tool};
use zeroize::Zeroizing;

use crate::env::Env;

/// Time limit for a credential `cmd`.
const CMD_DEADLINE: Deadline = Deadline(Duration::from_secs(10));

/// Reads credential values from the host environment, files and commands.
pub(crate) struct Creds {
    /// Host environment snapshot.
    env: Env,
    /// Spawner for `cmd` sources.
    proc: Arc<dyn Proc>,
}

impl Creds {
    /// Builds a reader over the host `env` and `proc`.
    pub(crate) fn new(env: Env, proc: Arc<dyn Proc>) -> Self {
        Self { env, proc }
    }

    /// Host environment snapshot.
    pub(crate) const fn env(&self) -> &Env {
        &self.env
    }

    /// Whether `src` yields a non-blank value.
    ///
    /// # Errors
    /// A moor-worded reason; never contains the value.
    pub(crate) fn check(&self, src: &CredentialSource) -> Result<(), String> {
        self.read(src).map(drop)
    }

    /// The non-blank credential bytes, wiped on drop.
    fn read(&self, src: &CredentialSource) -> Result<Zeroizing<Vec<u8>>, String> {
        let raw = self.fetch(src)?;
        if raw.iter().all(u8::is_ascii_whitespace) {
            return Err(blank_reason(src));
        }
        Ok(raw)
    }

    /// Raw bytes from whichever source is set.
    fn fetch(&self, src: &CredentialSource) -> Result<Zeroizing<Vec<u8>>, String> {
        if let Some(cmd) = &src.cmd {
            return self.run_cmd(cmd, &src.pass_env);
        }
        if let Some(file) = &src.file {
            return self.read_file(file);
        }
        if let Some(var) = &src.env {
            return self.env.get(var).map_or_else(
                || Err(format!("env {var} is not set")),
                |v| Ok(Zeroizing::new(v.as_bytes().to_vec())),
            );
        }
        Err("no source".to_owned())
    }

    /// Runs the command; stdout is the value.
    fn run_cmd(&self, cmd: &[String], pass_env: &[String]) -> Result<Zeroizing<Vec<u8>>, String> {
        let mut spec = ProcSpec::new(Tool::Sh, CMD_DEADLINE);
        for name in pass_env {
            if let Some(v) = self.env.get(name) {
                spec = spec.env(name.as_str(), v);
            }
        }
        let spec = spec.args(["-c", "exec \"$@\"", "moor-cred"]).args(cmd);
        match self.proc.run_secret(&spec) {
            Ok(out) => Ok(out),
            Err(ProcError::NonZero { code, .. }) => Err(format!("cmd exited {code}")),
            Err(ProcError::Signalled(_)) => Err("cmd was killed by a signal".to_owned()),
            Err(ProcError::Timeout { after, .. }) => {
                Err(format!("cmd timed out after {}s", after.as_secs()))
            }
            Err(ProcError::Spawn(e)) => Err(format!("cmd could not run: {}", e.kind())),
        }
    }

    /// Reads a regular file, expanding a leading `~`.
    fn read_file(&self, file: &Path) -> Result<Zeroizing<Vec<u8>>, String> {
        let path = self.env.expand_home(file);
        let meta = std::fs::metadata(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "not found".to_owned(),
            k => format!("unreadable: {k}"),
        })?;
        if !meta.is_file() {
            return Err("not a regular file".to_owned());
        }
        std::fs::read(&path)
            .map(Zeroizing::new)
            .map_err(|e| format!("unreadable: {}", e.kind()))
    }
}

/// Why a fetched value counted as blank.
fn blank_reason(src: &CredentialSource) -> String {
    if src.cmd.is_some() {
        "cmd printed nothing".to_owned()
    } else if src.file.is_some() {
        "file is empty".to_owned()
    } else {
        format!("env {} is empty", src.env.as_deref().unwrap_or_default())
    }
}

/// Source description safe to print: where the value comes from, never the value.
pub(crate) fn describe(src: &CredentialSource) -> String {
    if let Some(cmd) = &src.cmd {
        let prog = cmd.first().map_or("", String::as_str);
        if src.pass_env.is_empty() {
            return format!("cmd {prog}");
        }
        return format!("cmd {prog} (passes env {})", src.pass_env.join(", "));
    }
    if let Some(file) = &src.file {
        return format!("file {}", file.display());
    }
    if let Some(var) = &src.env {
        return format!("env {var}");
    }
    "no source".to_owned()
}

#[cfg(test)]
mod tests;
