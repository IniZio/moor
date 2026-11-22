//! Shell script text run on a Box. Pure: builds text, never runs it.

pub mod boxwork;
pub mod clone;
pub mod detached;
pub mod profile;
pub mod record;
pub mod resources;
pub mod setup;
pub mod teardown;

#[cfg(test)]
mod tests;

/// The line that puts the agent's install directories on the Box's `PATH`, in the launch script,
/// in the check that the agent command exists, and in the profile's `setup.sh`.
pub const AGENT_PATH_LINE: &str = "export PATH=\"$HOME/.local/bin:$HOME/.bun/bin:$PATH\"";

/// POSIX single-quotes `s`: wraps it in `'...'` and renders each `'` as `'\''`. Always quotes.
#[must_use]
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Runs its first argument under bash when the Box has bash, else under sh.
const BASH_OR_SH: &str =
    "if command -v bash >/dev/null 2>&1; then exec bash -c \"$1\"; fi; exec sh -c \"$1\"";

/// The argv that runs shell text `script` on a Box, under bash when the Box has it, else sh.
#[must_use]
pub fn shell_argv(script: &str) -> Vec<String> {
    ["sh", "-c", BASH_OR_SH, "sh", script]
        .map(str::to_owned)
        .to_vec()
}

/// Shell text that runs `argv` on a Box.
///
/// One element is a shell string and runs as `sh -c '<element>'`. Several elements are each
/// quoted and joined by spaces. An empty argv gives an empty string.
#[must_use]
pub fn command_line(argv: &[String]) -> String {
    match argv {
        [] => String::new(),
        [script] => format!("sh -c {}", quote(script)),
        _ => argv
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Seconds a run waits for an earlier copy before it goes on unlocked: as long as a setup exec
/// may take.
const LOCK_WAIT_SECS: u64 = 30 * 60;

/// `script` run with no other copy of it alongside in the same directory.
///
/// A copy a lost link left running in the Box is waited for, so a rerun finds its work done
/// instead of racing it. The lock is a `flock` on a file in `~/.moor` named after the directory,
/// held until the copy and every process it started have ended. A Box with no `flock` or no writable home runs
/// unlocked, and a wait longer than a setup exec may take goes on unlocked.
#[must_use]
pub fn one_at_a_time(script: &str) -> String {
    let secs = LOCK_WAIT_SECS;
    format!(
        r#"moor_lock="$HOME/.moor/one-at-a-time-$(pwd -P | cksum | cut -d ' ' -f 1).lock"
mkdir -p -- "$HOME/.moor" 2>/dev/null && : >>"$moor_lock" 2>/dev/null || moor_lock=/dev/null
(
[ "$moor_lock" = /dev/null ] || ! command -v flock >/dev/null 2>&1 || flock -w {secs} 9 || :
{script}
) 9>>"$moor_lock"
"#
    )
}
