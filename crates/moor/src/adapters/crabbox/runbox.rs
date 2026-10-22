//! A run detached in the Box, over the Box's shared ssh connection, never through `crabbox run`.
//!
//! Launch goes through the exec wrapper, so the run gets the env from stdin and the workdir
//! exactly as an exec does. Follow is one long ssh call that streams the run's log; the other
//! calls are short.

use std::io::Write;
use std::time::Duration;

use moor_core::boxscript::detached::{
    cancel_script, end_script, follow_script, launch_script, parse_end, LOST, REFUSED,
};
use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{
    Cancelled, DriverError, ExecOutcome, ExecSpec, FollowEnd, Followed, Launched, RunEnd,
};
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, Tool};

use super::errors::{one_line, EXIT_SSH};
use super::exec::{self, Wrapped};
use super::peek::ssh_target;
use super::{link, Crabbox};

/// Time allowed for a launch, an end read, or a cancel before its grace.
const SHORT: Duration = Duration::from_secs(30);

/// Starts `spec.argv` as run `run`, detached, inside the exec wrapper.
pub(super) fn launch(
    cb: &Crabbox,
    handle: &BoxHandle,
    run: i64,
    spec: &ExecSpec,
) -> Result<Launched, DriverError> {
    let command: Vec<String> = spec.argv.iter().map(|a| quote(a)).collect();
    let script = launch_script(run, &command.join(" "));
    let launcher = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), script],
        output: None,
        ..spec.clone()
    };
    let Wrapped { proc, exit_tag, .. } = exec::wrapped(cb, handle, &launcher, Deadline(SHORT))?;
    let e = match cb.proc.run(&proc) {
        Ok(out) => return started(&out.stdout),
        Err(e) => e,
    };
    let said = match &e {
        ProcError::NonZero { stderr_tail, .. } => own_lines(stderr_tail, &exit_tag),
        _ => String::new(),
    };
    match exec::outcome(cb, handle, Err(e), &exit_tag)? {
        ExecOutcome::Exited { code: REFUSED } => Ok(Launched::AlreadyLaunched),
        ExecOutcome::Exited { code } => Err(failed(
            format!("run {run} did not start (exit {code})"),
            &said,
        )),
        ExecOutcome::Interrupted => Err(failed(
            format!("the launch of run {run} was interrupted"),
            "",
        )),
    }
}

/// The pid the launch printed on its last line.
fn started(stdout: &str) -> Result<Launched, DriverError> {
    stdout
        .lines()
        .last()
        .and_then(|l| l.trim().parse().ok())
        .map(|pid| Launched::Started { pid })
        .ok_or_else(|| failed("the launch printed no pid".to_owned(), stdout))
}

/// The stderr lines the launch wrote itself, without the wrapper's marker lines.
fn own_lines(stderr: &str, exit_tag: &str) -> String {
    let token = exit_tag.trim_end_matches("-exit");
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.starts_with(token)).collect();
    lines.join("\n")
}

/// Writes run `run`'s log from `from` to `out` over one ssh call, touching the lease meanwhile.
pub(super) fn follow(
    cb: &Crabbox,
    handle: &BoxHandle,
    run: i64,
    from: u64,
    out: &mut dyn Write,
    deadline: Deadline,
) -> Result<Followed, DriverError> {
    let words = link::args(cb, handle, ssh_target(cb, handle)?)?;
    let spec = ProcSpec::new(Tool::Ssh, deadline)
        .args(words)
        .arg(format!("sh -c {}", quote(&follow_script(run, from))));
    let mut offset = from;
    let mut refused = None;
    let result = exec::touching_lease(cb, handle, || {
        cb.proc
            .run_reading(&spec, &mut |r| refused = copy(r, out, &mut offset).err())
    });
    if let Some(e) = refused {
        return Err(failed(
            format!("the output of run {run} could not be written: {e}"),
            "",
        ));
    }
    let end = match result {
        Ok(()) => FollowEnd::Exited,
        Err(ProcError::NonZero { code: LOST, .. }) => FollowEnd::Lost,
        Err(ProcError::Spawn(_)) => {
            return Err(DriverError::Unavailable(
                "ssh could not be started".to_owned(),
            ))
        }
        Err(ProcError::Timeout { .. } | ProcError::Interrupted | ProcError::Signalled(_)) => {
            FollowEnd::Cut
        }
        Err(e @ ProcError::NonZero { code: EXIT_SSH, .. }) => {
            link::failed(cb, handle, &e);
            FollowEnd::Cut
        }
        Err(ProcError::NonZero {
            code, stderr_tail, ..
        }) => {
            return Err(failed(
                format!("following run {run} failed (exit {code})"),
                &stderr_tail,
            ));
        }
    };
    Ok(Followed { offset, end })
}

/// Copies `r` to `out` until `r` ends or breaks, counting what was written in `offset`.
/// Only a refused write is an error: a broken read is the channel's, and ssh reports it.
fn copy(r: &mut dyn std::io::Read, out: &mut dyn Write, offset: &mut u64) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) | Err(_) => return out.flush(),
            Ok(n) => n,
        };
        out.write_all(buf.get(..n).unwrap_or_default())?;
        *offset += n as u64;
    }
}

/// Where run `run` stands, read in `workdir`.
pub(super) fn end(
    cb: &Crabbox,
    handle: &BoxHandle,
    run: i64,
    workdir: &BoxPath,
) -> Result<RunEnd, DriverError> {
    let script = format!(
        "cd -- {} || exit 1\n{}",
        quote(workdir.as_str()),
        end_script(run)
    );
    let spec = short(cb, handle, &script, SHORT)?;
    let out =
        link::run(cb, handle, &spec).map_err(|e| box_error(cb, handle, &e, "the run end read"))?;
    parse_end(&out.stdout)
        .ok_or_else(|| failed(format!("unreadable end of run {run}"), &out.stdout))
}

/// Stops run `run`'s process group, waiting `grace_secs` before KILL.
pub(super) fn cancel(
    cb: &Crabbox,
    handle: &BoxHandle,
    run: i64,
    grace_secs: u32,
) -> Result<Cancelled, DriverError> {
    let deadline = SHORT + Duration::from_secs(grace_secs.into());
    let spec = short(cb, handle, &cancel_script(run, grace_secs), deadline)?;
    match link::run(cb, handle, &spec) {
        Ok(_) => Ok(Cancelled::Signalled),
        Err(ProcError::NonZero { code: REFUSED, .. }) => Ok(Cancelled::NoProcess),
        Err(e) => Err(box_error(cb, handle, &e, "the run cancel")),
    }
}

/// The ssh call that runs `script` under sh in the Box's login directory on its shared connection.
fn short(
    cb: &Crabbox,
    handle: &BoxHandle,
    script: &str,
    deadline: Duration,
) -> Result<ProcSpec, DriverError> {
    let words = link::args(cb, handle, ssh_target(cb, handle)?)?;
    Ok(ProcSpec::new(Tool::Ssh, Deadline(deadline))
        .args(words)
        .arg(format!("sh -c {}", quote(script))))
}

/// The Driver error for a short call `what` that failed with `e`.
fn box_error(cb: &Crabbox, handle: &BoxHandle, e: &ProcError, what: &str) -> DriverError {
    match e {
        ProcError::Timeout { after, .. } => DriverError::Timeout(*after),
        ProcError::Spawn(_) => DriverError::Unavailable("ssh could not be started".to_owned()),
        ProcError::NonZero {
            code, stderr_tail, ..
        } => link::lost(cb, handle, e)
            .unwrap_or_else(|| failed(format!("{what} failed (exit {code})"), stderr_tail)),
        _ => failed(format!("{what} was interrupted"), ""),
    }
}

/// A [`DriverError::Failed`] with `summary`, then `detail` as one line when there is one.
fn failed(summary: String, detail: &str) -> DriverError {
    let detail = one_line(detail);
    DriverError::Failed {
        summary: if detail.is_empty() {
            summary
        } else {
            format!("{summary}: {detail}")
        },
    }
}

#[cfg(test)]
mod tests;
