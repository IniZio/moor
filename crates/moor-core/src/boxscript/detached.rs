//! Scripts for a run detached in the Box: launch, follow, end and cancel.
//!
//! A run keeps its files in `$HOME/.moor/runs/<id>/`, outside the workdir so a `git clean` in
//! the command cannot delete them: `log`, `start`, `pid` (the run's process group), `exit` and
//! `cancelled`. `run` is the run's id. Each script is POSIX sh text run with `sh -c` in the Box
//! workdir.
//!
//! A pid alone can name an unrelated process after a Box reboot or pid wraparound, so `start`
//! holds the boot id and the group leader's start time, and a pid counts as the run's only while
//! both still match. Both come from Linux `/proc`, which every Box has.

use super::{quote, record::MOVED_FILE};

/// Exit code of [`launch_script`] when the run dir already exists, and of [`cancel_script`]
/// when the pid file is missing or holds a pid unsafe to signal. Nothing was started or sent.
pub const REFUSED: i32 = 3;

/// Exit code of [`follow_script`] when the run's process is gone and left no exit code.
pub const LOST: i32 = 4;

/// Defines `__moor_ident PID`, which prints the boot id and the start time of process `PID`.
/// The start time is field 22 of `/proc/PID/stat`, counted after the last `)` because the
/// command name before it may hold spaces and `)`.
const IDENT_FN: &str = r#"__moor_ident() {
  __moor_boot=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null) || return 1
  __moor_stat=$(cat "/proc/$1/stat" 2>/dev/null) || return 1
  set -- ${__moor_stat##*)}
  [ -n "${20}" ] || return 1
  printf '%s %s\n' "$__moor_boot" "${20}"
}
"#;

/// Defines `__moor_alive`, true while the process `pid` names is the run's group leader: it
/// exists and its boot id and start time match `start`. Sets `__moor_pid`.
const ALIVE_FN: &str = r#"__moor_alive() {
  __moor_pid=$(cat "$__moor_dir/pid" 2>/dev/null) || return 1
  case "$__moor_pid" in ''|*[!0-9]*) return 1 ;; esac
  [ "$__moor_pid" -gt 1 ] || return 1
  __moor_want=$(cat "$__moor_dir/start" 2>/dev/null) || return 1
  __moor_have=$(__moor_ident "$__moor_pid") || return 1
  [ -n "$__moor_want" ] && [ "$__moor_have" = "$__moor_want" ]
}
"#;

/// Sets `__moor_dir` to the run dir of `run`.
fn dir_line(run: i64) -> String {
    format!("__moor_dir=\"$HOME/.moor/runs/{run}\"\n")
}

/// POSIX sh text that starts `command` (shell text) in its own session and prints its pid.
///
/// The command's stdout and stderr append to the run's `log`; its exit code lands in `exit`
/// once it ends. The script returns as soon as `pid` exists, and the command outlives the
/// calling shell and its ssh session.
#[must_use]
pub fn launch_script(run: i64, command: &str) -> String {
    // The detached shell writes its own pid, so `pid` names the group only once setsid is done.
    // `start` lands before `pid`, so a reader that sees `pid` can always check it.
    let inner = format!(
        r#"{IDENT_FN}__moor_dir=$1
shift
__moor_ident "$$" > "$__moor_dir/start.tmp" && mv -f "$__moor_dir/start.tmp" "$__moor_dir/start" || exit 1
printf '%s\n' "$$" > "$__moor_dir/pid.tmp" && mv -f "$__moor_dir/pid.tmp" "$__moor_dir/pid" || exit 1
(
{command}
)
__moor_rc=$?
printf '%s\n' "$__moor_rc" > "$__moor_dir/exit.tmp" && mv -f "$__moor_dir/exit.tmp" "$__moor_dir/exit"
"#
    );
    format!(
        r#"{dir}command -v setsid >/dev/null 2>&1 || {{ echo "moor: the Box has no setsid" >&2; exit 1; }}
mkdir -p "$HOME/.moor/runs" || exit 1
if ! mkdir "$__moor_dir" 2>/dev/null; then
  [ -e "$__moor_dir" ] && {{ echo "moor: run {run} was already launched" >&2; exit {REFUSED}; }}
  exit 1
fi
setsid sh -c {inner} sh "$__moor_dir" </dev/null >>"$__moor_dir/log" 2>&1 &
__moor_n=0
while [ ! -s "$__moor_dir/pid" ]; do
  [ "$__moor_n" -lt 100 ] || {{ echo "moor: run {run} did not start" >&2; exit 1; }}
  __moor_n=$((__moor_n + 1))
  sleep 0.1
done
cat "$__moor_dir/pid"
"#,
        dir = dir_line(run),
        inner = quote(&inner),
    )
}

/// POSIX sh text that prints the run's log from byte `offset`, byte for byte, as it grows.
///
/// Exits 0 once `exit` exists and the whole log is printed, or [`LOST`] once the process is
/// gone with no `exit`.
#[must_use]
pub fn follow_script(run: i64, offset: u64) -> String {
    // Liveness and `exit` are read before the log size, so the last pass prints everything.
    format!(
        r#"{dir}{IDENT_FN}{ALIVE_FN}__moor_off={offset}
while :; do
  __moor_end=
  __moor_alive || __moor_end=lost
  [ -e "$__moor_dir/exit" ] && __moor_end=exit
  __moor_size=0
  [ -e "$__moor_dir/log" ] && __moor_size=$(($(wc -c < "$__moor_dir/log")))
  if [ "$__moor_size" -gt "$__moor_off" ]; then
    tail -c +"$((__moor_off + 1))" "$__moor_dir/log" | head -c "$((__moor_size - __moor_off))" || exit 1
    __moor_off=$__moor_size
  fi
  case "$__moor_end" in
    exit) exit 0 ;;
    lost) exit {LOST} ;;
  esac
  sleep 0.2
done
"#,
        dir = dir_line(run),
    )
}

/// POSIX sh text that prints one line [`parse_end`] reads: the exit code (or `running` or
/// `lost`), whether a cancel was asked for, and whether the workdir holds [`MOVED_FILE`].
#[must_use]
pub fn end_script(run: i64) -> String {
    format!(
        r#"{dir}{IDENT_FN}{ALIVE_FN}__moor_exit=lost
__moor_alive && __moor_exit=running
if [ -e "$__moor_dir/exit" ]; then
  __moor_exit=$(cat "$__moor_dir/exit")
  case "$__moor_exit" in ''|*[!0-9]*) __moor_exit=lost ;; esac
fi
__moor_cancelled=0
[ -e "$__moor_dir/cancelled" ] && __moor_cancelled=1
__moor_moved=0
[ -e {MOVED_FILE} ] && __moor_moved=1
printf 'exit=%s cancelled=%s moved=%s\n' "$__moor_exit" "$__moor_cancelled" "$__moor_moved"
"#,
        dir = dir_line(run),
    )
}

/// POSIX sh text that marks the run `cancelled`, sends TERM to its process group, and KILL
/// once `grace_secs` pass with the group still alive.
///
/// Exits [`REFUSED`] without a signal when `pid` is missing, not a number, or 1 or less, since
/// `kill -- -1` would reach every process the Box user runs, and when the process it names is
/// not the run's group leader by `start`. Exits 0 when the group is gone.
#[must_use]
pub fn cancel_script(run: i64, grace_secs: u32) -> String {
    // dash rejects `kill -TERM -- -PID` as an illegal number, so the signal goes by `-s`.
    format!(
        r#"{dir}{IDENT_FN}{ALIVE_FN}__moor_pid=$(cat "$__moor_dir/pid" 2>/dev/null)
case "$__moor_pid" in
  ''|*[!0-9]*) echo "moor: run {run} has no pid to signal" >&2; exit {REFUSED} ;;
esac
[ "$__moor_pid" -gt 1 ] || {{ echo "moor: run {run} has pid $__moor_pid, refusing to signal" >&2; exit {REFUSED}; }}
__moor_alive || {{ echo "moor: pid $__moor_pid is not run {run}'s process, refusing to signal" >&2; exit {REFUSED}; }}
: > "$__moor_dir/cancelled" || exit 1
if ! kill -s TERM -- "-$__moor_pid"; then
  kill -s 0 -- "-$__moor_pid" 2>/dev/null || exit 0
  exit 1
fi
__moor_n=0
while kill -s 0 -- "-$__moor_pid" 2>/dev/null; do
  if [ "$__moor_n" -ge {ticks} ]; then
    kill -s KILL -- "-$__moor_pid" || exit 1
    break
  fi
  __moor_n=$((__moor_n + 1))
  sleep 0.1
done
"#,
        dir = dir_line(run),
        ticks = u64::from(grace_secs) * 10,
    )
}

/// Where a run stands, as [`end_script`] sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The command ended with this exit code.
    Exited(u8),
    /// The command is still running.
    Running,
    /// The process is gone and left no exit code.
    Lost,
}

/// What [`end_script`] reports about a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct End {
    /// Where the run stands.
    pub outcome: Outcome,
    /// Whether a cancel was asked for.
    pub cancelled: bool,
    /// Whether the workdir holds the recorded-run moved marker.
    pub moved: bool,
}

/// The report in the text [`end_script`] printed; `None` unless its first line is exactly
/// `exit=E cancelled=C moved=M`.
#[must_use]
pub fn parse_end(text: &str) -> Option<End> {
    let flag = |v: &str| match v {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    };
    let mut parts = text.lines().next()?.split(' ');
    let exit = parts.next()?.strip_prefix("exit=")?;
    let cancelled = flag(parts.next()?.strip_prefix("cancelled=")?)?;
    let moved = flag(parts.next()?.strip_prefix("moved=")?)?;
    if parts.next().is_some() {
        return None;
    }
    let outcome = match exit {
        "running" => Outcome::Running,
        "lost" => Outcome::Lost,
        n if n.bytes().all(|b| b.is_ascii_digit()) => Outcome::Exited(n.parse().ok()?),
        _ => return None,
    };
    Some(End {
        outcome,
        cancelled,
        moved,
    })
}

#[cfg(test)]
mod tests;
