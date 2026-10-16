//! Runs several Box steps over one ssh process, never through `crabbox run`.
//!
//! The connect command line from the lease is reused, so no `crabbox` process
//! starts. A POSIX sh script on the Box runs the steps and prints one framed
//! record per step; Put bytes go down ssh's stdin, so they never appear in an
//! argument list.

use std::path::Path;

use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{BoxStep, DriverError, ExecOutcome, StepDone};
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Stdin, Tool};

use super::peek::ssh_target;
use super::Crabbox;

/// Runs `steps` in order over one ssh process; see `Driver::batch` for the result shape.
/// `connect` is the argv from `connect_argv`; without a leading `ssh` the target is looked up
/// in `host_cwd`.
pub(super) fn run(
    cb: &Crabbox,
    handle: &BoxHandle,
    connect: &[String],
    steps: &[BoxStep],
    host_cwd: &Path,
    deadline: Deadline,
) -> Vec<Result<StepDone, DriverError>> {
    if steps.is_empty() {
        return Vec::new();
    }
    let words = match target_words(cb, handle, connect, host_cwd) {
        Ok(w) => w,
        Err(e) => return vec![Err(e)],
    };
    let spec = ProcSpec::new(Tool::Ssh, deadline)
        .args(words)
        .arg(format!("sh -c {}", quote(&script(steps))))
        .stdin(Stdin::Bytes(stdin_bytes(steps)));
    let (stdout, failure) = match cb.proc.run(&spec) {
        Ok(out) => (out.stdout, None),
        Err(e) => (String::new(), Some(e)),
    };
    let mut results = parse(stdout.as_bytes(), steps);
    if let Some(step) = steps.get(results.len()).filter(|_| !stopped(&results)) {
        results.push(Err(
            failure.map_or_else(|| failed(summary(step)), |e| ssh_failure(&e, step))
        ));
    }
    results
}

/// The ssh arguments for the Box: `connect` without `ssh` and `-t`, or a fresh lookup.
fn target_words(
    cb: &Crabbox,
    handle: &BoxHandle,
    connect: &[String],
    host_cwd: &Path,
) -> Result<Vec<String>, DriverError> {
    match connect.split_first() {
        Some((first, rest)) if first == "ssh" => {
            let rest = rest.strip_prefix(&["-t".to_owned()]).unwrap_or(rest);
            Ok(rest.to_vec())
        }
        _ => ssh_target(cb, handle, host_cwd),
    }
}

/// The sh script that runs `steps` and prints one frame per step.
///
/// Frames: `A` absent, `P <len>` file bytes, `E` read error, `R <code> <len>` command
/// output, `O` write done, `F` write failed. A length frame is followed by the bytes and a
/// newline. The script always exits 0 once it starts, so frames survive.
pub(super) fn script(steps: &[BoxStep]) -> String {
    let head = "T=$(mktemp -d) || exit 0\ntrap 'rm -rf \"$T\"' EXIT\n\
                len() { wc -c < \"$1\" | tr -d ' '; }\n";
    let body: String = steps.iter().map(step_script).collect();
    format!("{head}{body}exit 0\n")
}

/// The script text for one step.
fn step_script(step: &BoxStep) -> String {
    match step {
        BoxStep::Peek(path) => {
            let p = quote(path.as_str());
            format!(
                "if test -e {p}; then\n\
                 if cat -- {p} > \"$T/o\"; then echo \"P $(len \"$T/o\")\"; cat \"$T/o\"; echo\n\
                 else echo E; exit 0; fi\n\
                 else echo A; fi\n"
            )
        }
        BoxStep::Run(script) => {
            let q = quote(script);
            format!(
                "sh -c {q} </dev/null >\"$T/o\"; c=$?\n\
                 echo \"R $c $(len \"$T/o\")\"; cat \"$T/o\"; echo\n\
                 test \"$c\" -eq 0 || exit 0\n"
            )
        }
        BoxStep::Put { path, bytes, mode } => {
            let (dir, file) = (quote(dir_of(path)), quote(path.as_str()));
            let n = bytes.len();
            format!(
                "if ( umask 077 && mkdir -p {dir} && dd bs=1 count={n} of={file} 2>/dev/null \
                 && chmod {mode:o} {file} ); then echo O; else echo F; exit 0; fi\n"
            )
        }
    }
}

/// Directory part of `path`; `/` for a file at the root.
fn dir_of(path: &BoxPath) -> &str {
    match path.as_str().rsplit_once('/') {
        Some((d, _)) if !d.is_empty() => d,
        _ => "/",
    }
}

/// All Put bytes in step order.
fn stdin_bytes(steps: &[BoxStep]) -> Vec<u8> {
    steps
        .iter()
        .filter_map(|s| match s {
            BoxStep::Put { bytes, .. } => Some(bytes.as_slice()),
            _ => None,
        })
        .flatten()
        .copied()
        .collect()
}

/// Reads frames from `out`, one per step, until the steps end, a step stops the batch, or
/// the output runs out or is garbled.
fn parse(out: &[u8], steps: &[BoxStep]) -> Vec<Result<StepDone, DriverError>> {
    let mut r = Reader { buf: out, pos: 0 };
    let mut results = Vec::new();
    for step in steps {
        let Some(done) = frame(&mut r, step) else {
            break;
        };
        results.push(done);
        if stopped(&results) {
            break;
        }
    }
    results
}

/// Reads the frame for `step`; `None` when the output ends or does not match.
fn frame(r: &mut Reader<'_>, step: &BoxStep) -> Option<Result<StepDone, DriverError>> {
    let line = r.line()?;
    let mut f = line.split(' ');
    let tag = f.next()?;
    let mut num = || f.next()?.parse::<usize>().ok();
    Some(match (step, tag) {
        (BoxStep::Peek(_), "A") => Ok(StepDone::Peeked(None)),
        (BoxStep::Peek(_), "E") => Err(failed("box file read failed")),
        (BoxStep::Peek(_), "P") => {
            let body = r.body(num()?)?;
            Ok(StepDone::Peeked(Some(body)))
        }
        (BoxStep::Run(_), "R") => {
            let code = i32::try_from(num()?).ok()?;
            let stdout = r.body(num()?)?;
            Ok(StepDone::Ran {
                outcome: ExecOutcome::Exited { code },
                stdout,
            })
        }
        (BoxStep::Put { .. }, "O") => Ok(StepDone::Put),
        (BoxStep::Put { .. }, "F") => Err(failed("box file write failed")),
        _ => return None,
    })
}

/// Cursor over the ssh output.
struct Reader<'a> {
    /// Whole output.
    buf: &'a [u8],
    /// Next unread byte.
    pos: usize,
}

impl Reader<'_> {
    /// The next line without its newline.
    fn line(&mut self) -> Option<String> {
        let rest = self.buf.get(self.pos..)?;
        let end = rest.iter().position(|&b| b == b'\n')?;
        self.pos += end + 1;
        std::str::from_utf8(rest.get(..end)?)
            .ok()
            .map(str::to_owned)
    }

    /// `n` bytes plus the newline that follows them.
    fn body(&mut self, n: usize) -> Option<String> {
        let end = self.pos.checked_add(n)?;
        let bytes = self.buf.get(self.pos..end)?;
        if self.buf.get(end) != Some(&b'\n') {
            return None;
        }
        self.pos = end + 1;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// True when the last result ended the batch.
fn stopped(results: &[Result<StepDone, DriverError>]) -> bool {
    match results.last() {
        Some(Err(_)) => true,
        Some(Ok(StepDone::Ran { outcome, .. })) => *outcome != ExecOutcome::Exited { code: 0 },
        _ => false,
    }
}

/// A [`DriverError::Failed`] with `summary`.
fn failed(summary: &str) -> DriverError {
    DriverError::Failed {
        summary: summary.to_owned(),
    }
}

/// Failure summary by kind of step.
const fn summary(step: &BoxStep) -> &'static str {
    match step {
        BoxStep::Peek(_) => "box file read failed",
        BoxStep::Put { .. } => "box file write failed",
        BoxStep::Run(_) => "box command failed",
    }
}

/// The error for the first step that got no frame because ssh failed.
fn ssh_failure(e: &ProcError, step: &BoxStep) -> DriverError {
    match e {
        ProcError::Timeout { after, .. } => DriverError::Timeout(*after),
        ProcError::Spawn(_) => DriverError::Unavailable("ssh could not be started".to_owned()),
        _ => failed(summary(step)),
    }
}

#[cfg(test)]
mod tests;
