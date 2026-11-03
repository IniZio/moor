//! A child whose stdio streams to its caller byte for byte.

use std::process::Stdio;

use moor_core::seam::proc::{pump, Piped, Pipes, ProcError, ProcSpec, Stdin};

use super::SystemProc;

impl SystemProc {
    /// Runs `spec` with its stdio on `pipes`; plain `spec.stdin` bytes go first on its stdin.
    pub(super) fn piped(&self, spec: &ProcSpec, pipes: Pipes<'_>) -> Result<Piped, ProcError> {
        let preamble = match &spec.stdin {
            Stdin::Bytes(b) => b.clone(),
            _ => Vec::new(),
        };
        let child = self
            .command(spec)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(ProcError::Spawn)?;
        self.trace.started(spec);
        pump(child, preamble, pipes).map_err(ProcError::Spawn)
    }
}
