//! Runs one command in a Box over the Box's shared ssh connection, never through `crabbox run`.

use std::fmt::Write as _;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, ProcError, ProcSpec, StderrGate, Stdin, Tool};

use super::errors::{one_line, EXIT_SSH};
use super::peek::{lookup, ssh_target};
use super::{link, Crabbox};

/// How often a running Box command touches the Box's lease: crabbox renews a lease only
/// while one of its own commands runs, and moor's commands run over ssh instead.
pub(super) const TOUCH_EVERY: Duration = Duration::from_mins(10);

/// Shell wrapper the Box runs around the command.
///
/// `$1` and `$2` bracket the command's stderr, `$3` tags the line that reports its exit, and
/// `$4` is the directory to make and enter, or empty for the login directory. The rest is the
/// command. The env arrives on stdin, goes into a private temp file, is sourced and the file
/// removed before the command starts; the command reads `/dev/null`. The exit line is how moor
/// tells the command's own exit 255 from ssh losing the connection.
///
/// An EXIT trap holding the file's path also removes it, with an absolute `rm`, since a broken
/// env stream ends sh mid-source and an env may replace `PATH`. The path goes into the trap and
/// the positional words, never a variable the env could overwrite. The end marker starts with
/// a newline so it is a line of its own after stderr with no trailing newline.
pub(super) const WRAPPER: &str = r#"b=$1 e=$2 x=$3 d=$4; shift 4
f=$(mktemp) || exit 1
case $f in *"'"*) /bin/rm -f -- "$f"; exit 1;; esac
trap "/bin/rm -f -- '$f'" EXIT
set -- "$f" "$@"
cat >"$1" && . "$1"; r=$?; /bin/rm -f -- "$1"; shift
test "$r" -eq 0 || { echo "moor: the command env did not load" >&2; exit 1; }
printf '%s\n' "$b" >&2
if test -n "$d"; then mkdir -p -- "$d" && cd -- "$d" && "$@"; else "$@"; fi </dev/null
c=$?
printf '\n%s\n%s %s\n' "$e" "$x" "$c" >&2
exit "$c""#;

/// Runs `spec` in the Box behind `handle`.
///
/// With an `output` the command streams there and only the stderr between the wrapper's
/// markers is shown; without one it is captured and dropped.
pub(super) fn run(
    cb: &Crabbox,
    handle: &BoxHandle,
    spec: &ExecSpec,
    deadline: Deadline,
) -> Result<ExecOutcome, DriverError> {
    let Wrapped {
        proc,
        gate,
        exit_tag,
    } = wrapped(cb, handle, spec, deadline)?;
    let result = touching_lease(cb, handle, || match &spec.output {
        Some(out) => cb.proc.run_streamed(&proc.stderr_gate(gate), out),
        None => cb.proc.run(&proc).map(|_| ()),
    });
    outcome(cb, handle, result, &exit_tag)
}

/// An ssh call that runs a command inside [`WRAPPER`].
pub(super) struct Wrapped {
    /// The ssh call, with the env on its stdin.
    pub(super) proc: ProcSpec,
    /// The markers around the command's own stderr.
    pub(super) gate: StderrGate,
    /// The tag of the line that reports the command's exit.
    pub(super) exit_tag: String,
}

/// The ssh call on the Box's shared connection that runs `spec.argv` inside [`WRAPPER`].
pub(super) fn wrapped(
    cb: &Crabbox,
    handle: &BoxHandle,
    spec: &ExecSpec,
    deadline: Deadline,
) -> Result<Wrapped, DriverError> {
    if spec.argv.is_empty() {
        return Err(DriverError::Failed {
            summary: "the box command is empty".to_owned(),
        });
    }
    let env = env_file(&spec.env)?;
    let words = link::args(cb, handle, ssh_target(cb, handle)?)?;
    let token = format!("gate-{:08x}{:08x}", cb.rng.u32(), cb.rng.u32());
    let gate = StderrGate {
        begin: format!("{token}-begin"),
        end: format!("{token}-end"),
    };
    let exit_tag = format!("{token}-exit");
    let cwd = spec.cwd.as_ref().map_or("", |c| c.as_str());
    let mut remote = format!("sh -c {} sh", quote(WRAPPER));
    for word in [&gate.begin, &gate.end, &exit_tag, cwd]
        .into_iter()
        .chain(spec.argv.iter().map(String::as_str))
    {
        remote.push(' ');
        remote.push_str(&quote(word));
    }
    let proc = ProcSpec::new(Tool::Ssh, deadline)
        .args(words)
        .arg(remote)
        .stdin(Stdin::Bytes(env));
    Ok(Wrapped {
        proc,
        gate,
        exit_tag,
    })
}

impl Crabbox {
    /// Touches the lease of a Box running a command every `every`, in place of [`TOUCH_EVERY`].
    #[cfg(test)]
    pub(crate) const fn with_touch_every(mut self, every: Duration) -> Self {
        self.touch_every = every;
        self
    }
}

/// Runs `command`, and while it runs touches the lease of the Box behind `handle` every
/// `cb.touch_every` with a fresh lookup, so crabbox does not reap a Box that is busy with a
/// long command. A failed touch is left to the next one.
pub(super) fn touching_lease<T>(
    cb: &Crabbox,
    handle: &BoxHandle,
    command: impl FnOnce() -> T,
) -> T {
    let (ended, wait) = mpsc::channel::<()>();
    thread::scope(|s| {
        s.spawn(move || {
            while wait.recv_timeout(cb.touch_every) == Err(RecvTimeoutError::Timeout) {
                let _ = lookup(cb, handle);
            }
        });
        let out = command();
        drop(ended);
        out
    })
}

/// The env as sh `export` lines, refused when a name is not a shell name.
fn env_file(env: &[(String, String)]) -> Result<Vec<u8>, DriverError> {
    let mut text = String::new();
    for (k, v) in env {
        let mut chars = k.chars();
        let named = chars
            .next()
            .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
            && chars.all(|c| c == '_' || c.is_ascii_alphanumeric());
        if !named {
            return Err(DriverError::Failed {
                summary: format!("env name {k:?} is not a shell variable name"),
            });
        }
        let _ = writeln!(text, "export {k}={}", quote(v));
    }
    Ok(text.into_bytes())
}

/// How the exec ended, from ssh's result and the exit line tagged `exit_tag`. A failure that is
/// not the command's own exit goes to [`link::failed`].
pub(super) fn outcome(
    cb: &Crabbox,
    handle: &BoxHandle,
    result: Result<(), ProcError>,
    exit_tag: &str,
) -> Result<ExecOutcome, DriverError> {
    let e = match result {
        Ok(()) => return Ok(ExecOutcome::Exited { code: 0 }),
        Err(e) => e,
    };
    if let ProcError::NonZero { stderr_tail, .. } = &e {
        if let Some(code) = reported_exit(stderr_tail, exit_tag) {
            return Ok(ExecOutcome::Exited { code });
        }
    }
    link::failed(cb, handle, &e);
    match e {
        ProcError::Interrupted | ProcError::Signalled(_) => Ok(ExecOutcome::Interrupted),
        ProcError::Timeout { after, .. } => Err(DriverError::Timeout(after)),
        ProcError::Spawn(_) => Err(DriverError::Unavailable(
            "ssh could not be started".to_owned(),
        )),
        ProcError::NonZero {
            code: EXIT_SSH,
            ref stderr_tail,
            ..
        } => Err(
            link::lost(cb, handle, &e).unwrap_or_else(|| DriverError::Transport {
                stderr_tail: one_line(stderr_tail),
            }),
        ),
        ProcError::NonZero {
            code, stderr_tail, ..
        } => {
            let tail = one_line(&stderr_tail);
            let tail = if tail.is_empty() {
                String::new()
            } else {
                format!(": {tail}")
            };
            Err(DriverError::Failed {
                summary: format!("box command did not start (exit {code}){tail}"),
            })
        }
    }
}

/// The exit code the wrapper reported on its line tagged `exit_tag`, the last such line winning.
fn reported_exit(stderr: &str, exit_tag: &str) -> Option<i32> {
    stderr.lines().rev().find_map(|l| {
        l.trim_end_matches('\r')
            .strip_prefix(exit_tag)?
            .strip_prefix(' ')?
            .parse()
            .ok()
    })
}

#[cfg(test)]
mod tests;
