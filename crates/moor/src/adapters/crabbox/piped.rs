//! Runs one command in a Box with its stdio streamed byte for byte over ssh, without a terminal
//! or an escape character, so no byte the caller sends is taken as an ssh command.

use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Piped, Pipes, ProcError, ProcSpec, Stdin, Tool};

use super::exec::{env_file, touching_lease};
use super::peek::ssh_target;
use super::{link, Crabbox, DIAL_WITHIN};

/// How long the ssh dial and the kill of a left command may take.
const SHORT: Duration = Duration::from_secs(30);

/// Shell wrapper the Box runs around the command.
///
/// `$1` is the length of the env that comes first on stdin, `$2` the directory to make and
/// enter or empty, `$3` the file that records the command's process group; the rest is the
/// command. `dd` reads the env a byte at a time, so not one byte of the caller's stdin is
/// taken with it. sshd starts a session without a terminal in a process group of its own and
/// does not signal it when the client goes, so moor kills the recorded group itself.
const WRAPPER: &str = r#"n=$1 d=$2 p=$3; shift 3
f=$(mktemp) || exit 1
case $f in *"'"*) /bin/rm -f -- "$f"; exit 1;; esac
trap "/bin/rm -f -- '$f' '$p'" EXIT
read -r _ _ _ _ g _ </proc/$$/stat && echo "$g" >"$p" || exit 1
if test -n "$d"; then mkdir -p -- "$d" && cd -- "$d" || exit 1; fi
set -- "$f" "$@"
dd bs=1 count="$n" of="$1" 2>/dev/null && . "$1"; r=$?; /bin/rm -f -- "$1"; shift
test "$r" -eq 0 || { echo "moor: the command env did not load" >&2; exit 1; }
"$@""#;

/// Runs `spec` in the Box behind `handle` with its stdio on `pipes`.
pub(super) fn run(
    cb: &Crabbox,
    handle: &BoxHandle,
    spec: &ExecSpec,
    pipes: Pipes<'_>,
) -> Result<ExecOutcome, DriverError> {
    if spec.argv.is_empty() {
        return Err(DriverError::Failed {
            summary: "the box command is empty".to_owned(),
        });
    }
    let env = env_file(&spec.env)?;
    let group = format!("/tmp/moor-exec-{:08x}{:08x}", cb.rng.u32(), cb.rng.u32());
    let cwd = spec.cwd.as_ref().map_or("", |c| c.as_str());
    let mut remote = format!("sh -c {} sh", quote(WRAPPER));
    for word in [env.len().to_string().as_str(), cwd, &group]
        .into_iter()
        .chain(spec.argv.iter().map(String::as_str))
    {
        remote.push(' ');
        remote.push_str(&quote(word));
    }
    let proc = ProcSpec::new(Tool::Ssh, Deadline(SHORT))
        .env(DIAL_WITHIN, SHORT.as_secs().to_string())
        .args(["-T", "-e", "none", "-o", "BatchMode=yes"])
        .args(link::args(cb, handle, ssh_target(cb, handle)?)?)
        .arg(remote)
        .stdin(Stdin::Bytes(env));
    let ended = touching_lease(cb, handle, || cb.proc.run_piped(&proc, pipes));
    match ended {
        Ok(Piped {
            code: Some(code),
            stopped: false,
        }) => Ok(ExecOutcome::Exited { code }),
        Ok(Piped { stopped, .. }) => {
            if stopped {
                kill(cb, handle, &group);
            }
            Ok(ExecOutcome::Interrupted)
        }
        Err(ProcError::Spawn(e)) => Err(DriverError::Unavailable(format!(
            "ssh could not be started: {e}"
        ))),
        Err(e) => Err(DriverError::Failed {
            summary: e.to_string(),
        }),
    }
}

/// Hangs up the process group recorded in `group`, whose client went away; a failure leaves the
/// command to end on its own.
fn kill(cb: &Crabbox, handle: &BoxHandle, group: &str) {
    let script = kill_script(group);
    let Ok(words) = ssh_target(cb, handle).and_then(|t| link::args(cb, handle, t)) else {
        return;
    };
    let spec = ProcSpec::new(Tool::Ssh, Deadline(SHORT))
        .args(words)
        .arg(format!("sh -c {}", quote(&script)));
    let _ = link::run(cb, handle, &spec);
}

/// Shell that hangs up the process group recorded in `group` and removes the record.
fn kill_script(group: &str) -> String {
    format!(
        "g=$(cat {f} 2>/dev/null) && kill -s HUP -- -$g; rm -f {f}",
        f = quote(group)
    )
}

#[cfg(test)]
mod tests;
