//! Files, batches and detached runs composed over [`Driver::exec_piped`], so a machine that only
//! runs commands in a Box place serves them all. Each call is one `exec` stream.

use std::io::{self, Cursor, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use moor_core::boxscript::detached::{
    cancel_script, end_script, follow_script, launch_script, parse_end, LOST, REFUSED,
};
use moor_core::boxscript::quote;
use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::driver::{
    BoxStep, Cancelled, Driver, DriverError, ExecOutcome, ExecSpec, FollowEnd, Followed, Launched,
    Place, RunEnd, StepDone,
};
use moor_core::seam::proc::{Deadline, Pipes, OUTPUT_CAP};

use super::exec::SHORT;
use super::relink::relinked;
use super::stream::{failed, Timer};

pub(super) mod copy;

/// Exit code of a read that found no file.
const MISSING: i32 = 44;
/// Time allowed for a file read.
const READ_WITHIN: Duration = Duration::from_secs(5);
/// Time allowed for a file write.
const WRITE_WITHIN: Duration = Duration::from_secs(30);

/// A writer that keeps what fits in the capture cap and discards the rest.
struct Capped(Vec<u8>);

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let room = OUTPUT_CAP.saturating_sub(self.0.len());
        self.0
            .extend_from_slice(buf.get(..buf.len().min(room)).unwrap_or_default());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs `spec` with `stdin` for its stdin, within `within`; how it ended and its stdout.
fn capture(
    d: &dyn Driver,
    h: &BoxHandle,
    spec: &ExecSpec,
    stdin: Vec<u8>,
    within: Duration,
) -> Result<(ExecOutcome, String), DriverError> {
    let (mut out, mut err) = (Capped(Vec::new()), Capped(Vec::new()));
    let timer = Timer::after(within);
    let stop = move || timer.is_up();
    let pipes = Pipes {
        stdin: Box::new(Cursor::new(stdin)),
        stdout: &mut out,
        stderr: &mut err,
        stop: &stop,
    };
    let outcome = d.exec_piped(h, spec, pipes)?;
    if outcome == ExecOutcome::Interrupted && timer.is_up() {
        return Err(DriverError::Timeout(within));
    }
    Ok((outcome, String::from_utf8_lossy(&out.0).into_owned()))
}

/// [`capture`] for a command safe to run twice, run again while the machine's link is down.
fn capture_again(
    d: &dyn Driver,
    h: &BoxHandle,
    spec: &ExecSpec,
    stdin: &[u8],
    within: Duration,
) -> Result<(ExecOutcome, String), DriverError> {
    relinked(|| capture(d, h, spec, stdin.to_vec(), within))
}

/// Runs `spec` with no stdin; its stdout and how it ended.
pub(super) fn capture_out(
    d: &dyn Driver,
    h: &BoxHandle,
    spec: &ExecSpec,
) -> Result<(String, ExecOutcome), DriverError> {
    capture(d, h, spec, Vec::new(), SHORT).map(|(outcome, out)| (out, outcome))
}

/// `script` run by `sh -c` in `place`, in the Box's workdir.
fn sh(place: Place, script: &str) -> ExecSpec {
    ExecSpec::new(
        place,
        vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
    )
}

/// The directory part of `path`; `/` for a file at the root.
fn dir_of(path: &BoxPath) -> &str {
    match path.as_str().rsplit_once('/') {
        Some((d, _)) if !d.is_empty() => d,
        _ => "/",
    }
}

/// Reads `path` in `place`; `None` when it does not exist.
pub(super) fn peek_file(
    d: &dyn Driver,
    h: &BoxHandle,
    place: Place,
    path: &BoxPath,
) -> Result<Option<String>, DriverError> {
    let p = quote(path.as_str());
    let spec = sh(place, &format!("test -e {p} || exit {MISSING}; cat -- {p}"));
    match capture_again(d, h, &spec, &[], READ_WITHIN)? {
        (ExecOutcome::Exited { code: 0 }, text) => Ok(Some(text)),
        (ExecOutcome::Exited { code: MISSING }, _) => Ok(None),
        _ => Err(failed("box file read failed".to_owned())),
    }
}

/// Writes `bytes` to `path` in `place` and leaves it at `mode`; the content travels on stdin.
pub(super) fn put_file(
    d: &dyn Driver,
    h: &BoxHandle,
    place: Place,
    path: &BoxPath,
    (bytes, mode): (&[u8], u32),
) -> Result<(), DriverError> {
    let (dir, file) = (quote(dir_of(path)), quote(path.as_str()));
    // umask keeps the file private while it is written, chmod then sets the real mode, and the
    // write replaces the whole file, so a repeat is safe.
    let script = format!("umask 077 && mkdir -p {dir} && cat > {file} && chmod {mode:o} {file}");
    match capture_again(d, h, &sh(place, &script), bytes, WRITE_WITHIN)? {
        (ExecOutcome::Exited { code: 0 }, _) => Ok(()),
        _ => Err(failed("box file write failed".to_owned())),
    }
}

/// Runs `steps` in `place` one stream each, stopping after a step that errs or a `Run` that does
/// not exit 0, so the result holds one entry per step run.
pub(super) fn batch(
    d: &dyn Driver,
    h: &BoxHandle,
    place: Place,
    steps: &[BoxStep],
    deadline: Deadline,
) -> Vec<Result<StepDone, DriverError>> {
    let Deadline(within) = deadline;
    let mut done = Vec::with_capacity(steps.len());
    for step in steps {
        let result = match step {
            BoxStep::Peek(path) => peek_file(d, h, place, path).map(StepDone::Peeked),
            BoxStep::Put { path, bytes, mode } => {
                put_file(d, h, place, path, (bytes, *mode)).map(|()| StepDone::Put)
            }
            BoxStep::Run(script) => capture(d, h, &sh(place, script), Vec::new(), within)
                .map(|(outcome, stdout)| StepDone::Ran { outcome, stdout }),
            BoxStep::Repeatable(script) => capture_again(d, h, &sh(place, script), &[], within)
                .map(|(outcome, stdout)| StepDone::Ran { outcome, stdout }),
        };
        let stop = match &result {
            Err(_) => true,
            Ok(StepDone::Ran { outcome, .. }) => *outcome != ExecOutcome::Exited { code: 0 },
            Ok(_) => false,
        };
        done.push(result);
        if stop {
            break;
        }
    }
    done
}

/// Starts `spec.argv` as run `run`, detached in the work place, with `spec`'s env and cwd as an
/// exec has them.
pub(super) fn launch_run(
    d: &dyn Driver,
    h: &BoxHandle,
    run: i64,
    spec: &ExecSpec,
) -> Result<Launched, DriverError> {
    let command: Vec<String> = spec.argv.iter().map(|a| quote(a)).collect();
    let launcher = ExecSpec {
        argv: sh(Place::Work, &launch_script(run, &command.join(" "))).argv,
        place: Place::Work,
        output: None,
        ..spec.clone()
    };
    // A second launch of the run is refused, so a repeat is safe.
    match capture_again(d, h, &launcher, &[], SHORT)? {
        (ExecOutcome::Exited { code: 0 }, out) => out
            .lines()
            .last()
            .and_then(|l| l.trim().parse().ok())
            .map(|pid| Launched::Started { pid })
            .ok_or_else(|| failed(format!("run {run} printed no pid"))),
        (ExecOutcome::Exited { code: REFUSED }, _) => Ok(Launched::AlreadyLaunched),
        (ExecOutcome::Exited { code }, _) => {
            Err(failed(format!("run {run} did not start (exit {code})")))
        }
        (ExecOutcome::Interrupted, _) => {
            Err(failed(format!("the launch of run {run} was interrupted")))
        }
    }
}

/// A writer that hands what it is given to the thread following the run.
struct Relay(mpsc::Sender<Vec<u8>>);

impl Write for Relay {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .send(buf.to_vec())
            .map_err(|_| io::Error::other("the follower is gone"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes run `run`'s log to `out` from byte `from` as it grows, until the run ends, its process
/// is lost, or the stream is cut or outlives `deadline`. The command runs on a thread of its own,
/// so `out` need not be `Send`.
pub(super) fn follow_run(
    d: &dyn Driver,
    h: &BoxHandle,
    run: i64,
    from: u64,
    out: &mut dyn Write,
    deadline: Deadline,
) -> Result<Followed, DriverError> {
    let Deadline(within) = deadline;
    let timer = Timer::after(within);
    let refused = AtomicBool::new(false);
    let stop = || refused.load(Ordering::Relaxed) || timer.is_up();
    let (tx, rx) = mpsc::channel();
    let spec = sh(Place::Work, &follow_script(run, from));
    let (outcome, offset, broke) = std::thread::scope(|s| {
        let job = s.spawn(|| {
            let (mut relay, mut err) = (Relay(tx), Capped(Vec::new()));
            let pipes = Pipes {
                stdin: Box::new(io::empty()),
                stdout: &mut relay,
                stderr: &mut err,
                stop: &stop,
            };
            d.exec_piped(h, &spec, pipes)
        });
        let (mut offset, mut broke) = (from, None);
        for chunk in rx {
            if broke.is_none() {
                match out.write_all(&chunk) {
                    Ok(()) => offset += chunk.len() as u64,
                    Err(e) => {
                        refused.store(true, Ordering::Relaxed);
                        broke = Some(e);
                    }
                }
            }
        }
        let _ = out.flush();
        let outcome = job
            .join()
            .unwrap_or_else(|_| Err(failed("the follow panicked".to_owned())));
        (outcome, offset, broke)
    });
    if let Some(e) = broke {
        return Err(failed(format!(
            "the output of run {run} could not be written: {e}"
        )));
    }
    let end = match outcome {
        Ok(ExecOutcome::Exited { code: 0 }) => FollowEnd::Exited,
        Ok(ExecOutcome::Exited { code: LOST }) => FollowEnd::Lost,
        Ok(ExecOutcome::Exited { code }) => {
            return Err(failed(format!("following run {run} failed (exit {code})")));
        }
        Ok(ExecOutcome::Interrupted) | Err(DriverError::Transport { .. }) => FollowEnd::Cut,
        Err(e) => return Err(e),
    };
    Ok(Followed { offset, end })
}

/// Where run `run` stands, read in `workdir`, or the login directory without one.
pub(super) fn end_run(
    d: &dyn Driver,
    h: &BoxHandle,
    run: i64,
    workdir: Option<&BoxPath>,
) -> Result<RunEnd, DriverError> {
    let cd = workdir.map_or_else(String::new, |w| {
        format!("cd -- {} || exit 1\n", quote(w.as_str()))
    });
    let spec = sh(Place::Work, &format!("{cd}{}", end_script(run)));
    match capture_again(d, h, &spec, &[], SHORT)? {
        (ExecOutcome::Exited { code: 0 }, text) => parse_end(&text)
            .ok_or_else(|| failed(format!("unreadable end of run {run}: {}", text.trim()))),
        _ => Err(failed("the run end read failed".to_owned())),
    }
}

/// Stops run `run`'s process group, waiting `grace_secs` before KILL.
pub(super) fn cancel_run(
    d: &dyn Driver,
    h: &BoxHandle,
    run: i64,
    grace_secs: u32,
) -> Result<Cancelled, DriverError> {
    let within = SHORT + Duration::from_secs(grace_secs.into());
    let spec = sh(Place::Work, &cancel_script(run, grace_secs));
    match capture_again(d, h, &spec, &[], within)? {
        (ExecOutcome::Exited { code: 0 }, _) => Ok(Cancelled::Signalled),
        (ExecOutcome::Exited { code: REFUSED }, _) => Ok(Cancelled::NoProcess),
        _ => Err(failed("the run cancel failed".to_owned())),
    }
}
