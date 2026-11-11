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
use super::peek::ssh_target;
use super::{link, touch, Crabbox};

/// How often a running Box command touches the Box's lease: crabbox renews a lease only
/// while one of its own commands runs, and moor's commands run over ssh instead.
pub(super) const TOUCH_EVERY: Duration = Duration::from_mins(10);

/// Shell wrapper the Box runs around the command.
///
/// `$1` and `$2` bracket the command's stderr, `$3` tags the line that reports its exit, `$4`
/// is the directory to make and enter, or empty for the login directory, and `$5` the deadline
/// in seconds. The rest is the command. The env arrives on stdin, goes into a private temp file,
/// is sourced and the file removed before the command starts; the command reads `/dev/null`.
/// The exit line is how moor tells the command's own exit 255 from ssh losing the connection.
///
/// An EXIT trap holding the file's path also removes it, with an absolute `rm`, since a broken
/// env stream ends sh mid-source and an env may replace `PATH`. The path goes into the trap and
/// the positional words, never a variable the env could overwrite. The end marker starts with
/// a newline so it is a line of its own after stderr with no trailing newline.
///
/// sshd does not signal a command when its client goes, so the Box ends it at the deadline
/// itself: `timeout` runs it in a process group of its own and hangs up the whole group, killing
/// it 10 s later. The inner sh writes the exit code to fd 3 only when the command ended on its
/// own, so an empty code means the deadline ended it, reported as `timeout` on the exit line.
pub(super) const WRAPPER: &str = r#"b=$1 e=$2 x=$3 d=$4 t=$5; shift 5
f=$(mktemp) || exit 1
case $f in *"'"*) /bin/rm -f -- "$f"; exit 1;; esac
trap "/bin/rm -f -- '$f'" EXIT
set -- "$f" "$@"
cat >"$1" && . "$1"; r=$?; /bin/rm -f -- "$1"; shift
test "$r" -eq 0 || { echo "moor: the command env did not load" >&2; exit 1; }
printf '%s\n' "$b" >&2
{ c=$({ if test -n "$d"; then mkdir -p -- "$d" && cd -- "$d" || { echo $? >&3; exit; }; fi
/usr/bin/timeout -k 10 "$t" /bin/sh -c '"$@" 3>&-; echo $? >&3' sh "$@"
} 3>&1 >&4 4>&- </dev/null); } 4>&1
printf '\n%s\n' "$e" >&2
test -n "$c" || { printf '%s timeout\n' "$x" >&2; exit 124; }
printf '%s %s\n' "$x" "$c" >&2
exit "$c""#;

/// How long past its deadline the ssh call of a command may run: the Box's 10 s between hanging
/// up and killing the command, and the ssh round trip that reports it.
const GRACE: Duration = Duration::from_secs(30);

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
    outcome(cb, handle, result, &exit_tag, deadline)
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
    let within = (deadline.0.as_secs() + u64::from(deadline.0.subsec_nanos() > 0))
        .max(1)
        .to_string();
    let mut remote = format!("sh -c {} sh", quote(WRAPPER));
    for word in [&gate.begin, &gate.end, &exit_tag, cwd, &within]
        .into_iter()
        .chain(spec.argv.iter().map(String::as_str))
    {
        remote.push(' ');
        remote.push_str(&quote(word));
    }
    let proc = ProcSpec::new(Tool::Ssh, Deadline(deadline.0.saturating_add(GRACE)))
        .env(super::DIAL_WITHIN, within)
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

/// Runs `command`, touching the lease of the Box behind `handle` as it starts and every
/// `cb.touch_every` while it runs, so crabbox does not reap a Box that is busy with a long
/// command. A failed touch is left to the next one.
pub(super) fn touching_lease<T>(
    cb: &Crabbox,
    handle: &BoxHandle,
    command: impl FnOnce() -> T,
) -> T {
    let _ = touch::heartbeat(cb, handle);
    while_touching(cb, handle, command)
}

/// Runs `command`, touching the lease of the Box behind `handle` every `cb.touch_every` while
/// it runs.
pub(super) fn while_touching<T>(
    cb: &Crabbox,
    handle: &BoxHandle,
    command: impl FnOnce() -> T,
) -> T {
    let (ended, wait) = mpsc::channel::<()>();
    thread::scope(|s| {
        s.spawn(move || {
            while wait.recv_timeout(cb.touch_every) == Err(RecvTimeoutError::Timeout) {
                let _ = touch::send(cb, handle);
            }
        });
        let out = command();
        drop(ended);
        out
    })
}

/// The env as sh `export` lines, refused when a name is not a shell name.
pub(super) fn env_file(env: &[(String, String)]) -> Result<Vec<u8>, DriverError> {
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

/// How the exec ended, from ssh's result and the exit line tagged `exit_tag`; `deadline` is the
/// one the wrapper ended the command at. A failure that is not the command's own exit goes to
/// [`link::failed`].
pub(super) fn outcome(
    cb: &Crabbox,
    handle: &BoxHandle,
    result: Result<(), ProcError>,
    exit_tag: &str,
    deadline: Deadline,
) -> Result<ExecOutcome, DriverError> {
    let e = match result {
        Ok(()) => return Ok(ExecOutcome::Exited { code: 0 }),
        Err(e) => e,
    };
    if let ProcError::NonZero { stderr_tail, .. } = &e {
        match reported_exit(stderr_tail, exit_tag) {
            Some(Reported::Exit(code)) => return Ok(ExecOutcome::Exited { code }),
            Some(Reported::PastDeadline) => return Err(DriverError::Timeout(deadline.0)),
            None => {}
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

/// How the wrapper said the command ended.
#[derive(Debug, PartialEq, Eq)]
enum Reported {
    /// The command exited with this code.
    Exit(i32),
    /// The wrapper ended the command at its deadline.
    PastDeadline,
}

/// What the wrapper reported on its line tagged `exit_tag`, the last such line winning.
fn reported_exit(stderr: &str, exit_tag: &str) -> Option<Reported> {
    stderr.lines().rev().find_map(|l| {
        match l
            .trim_end_matches('\r')
            .strip_prefix(exit_tag)?
            .strip_prefix(' ')?
        {
            "timeout" => Some(Reported::PastDeadline),
            code => code.parse().ok().map(Reported::Exit),
        }
    })
}

#[cfg(test)]
mod tests;
