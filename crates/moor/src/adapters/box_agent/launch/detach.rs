//! The agent runs in a detached tmux session on the Box and the host pane attaches to it, so a
//! dropped ssh link leaves the agent running and the pane reconnects.

use std::time::Duration;

use moor_core::boxscript::{command_line, quote};
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::{EngineError, SessionRef};
use moor_core::seam::proc::Deadline;

/// The tmux session the agent runs in; a Box runs one agent at a time.
pub(in crate::adapters::box_agent) const SESSION: &str = "moor-agent";
/// Where the agent's exit status lands, relative to the Box workdir.
const EXIT_FILE: &str = ".moor/agent.exit";
/// Reconnects in a row before the pane gives up on a lost link; the waits double from 1 s.
const RECONNECTS: u32 = 5;
/// An attach that lasted this many seconds resets the reconnect count.
const SETTLED_SECS: u32 = 60;

/// The Box command that attaches to the agent's session and exits with the agent's status once
/// the session is over, or 255 while the session still runs, so the pane attaches again.
fn attach(exit_file: &str) -> String {
    format!(
        "if tmux has-session -t {SESSION} 2>/dev/null; then tmux attach-session -t {SESSION}; fi; \
         if tmux has-session -t {SESSION} 2>/dev/null; then exit 255; fi; \
         if [ -s {exit_file} ]; then exit \"$(cat {exit_file})\"; fi; exit 1"
    )
}

/// The Box command that replaces any agent session with one running the script file at
/// `script`, then attaches. A Box without tmux refuses to start the agent: on the bare link a
/// dropped connection would end it again.
fn start(exit_file: &str, script: &str) -> String {
    let inner = format!("sh {}; echo $? > {exit_file}", quote(script));
    format!(
        "if ! command -v tmux >/dev/null 2>&1; then \
         echo '{NO_TMUX}' >&2; exit {NO_TMUX_EXIT}; fi; \
         tmux kill-session -t {SESSION} 2>/dev/null; rm -f {exit_file}; \
         tmux new-session -d -s {SESSION} -x 200 -y 50 {} || exit 1; \
         tmux set-option -t {SESSION} status off >/dev/null; \
         tmux set-option -t {SESSION} prefix None >/dev/null; \
         tmux set-option -s escape-time 0 >/dev/null; {}",
        quote(&inner),
        attach(exit_file),
    )
}

/// What a Box without tmux prints before refusing to start the agent.
const NO_TMUX: &str =
    "moor: tmux is not installed on the Box; the agent needs it to outlive a dropped link";
/// The exit status of that refusal, the shell's command-not-found status.
const NO_TMUX_EXIT: u8 = 127;

/// The host pane's shell line: start the agent script at `script` on the Box through `connect`,
/// reconnect with doubling waits while ssh reports a lost link (255), then print the exit marker
/// with the agent's status. The script travels as a file, not inside this line: each shell layer
/// between here and tmux would quote it again.
pub(super) fn pane_line(
    connect: &[String],
    workdir: &str,
    script: &str,
) -> Result<String, NameError> {
    let exit_file = quote(BoxPath::under(workdir, EXIT_FILE)?.as_str());
    let ssh = command_line(connect);
    Ok(format!(
        "{ssh} {}; c=$?; i=0; \
         while [ \"$c\" -eq 255 ] && [ \"$i\" -lt {RECONNECTS} ]; do \
         printf '\\r\\033[Kmoor: link to the Box lost, reconnecting\\n'; sleep $((1 << i)); \
         t=$(date +%s); {ssh} {}; c=$?; \
         if [ $(($(date +%s) - t)) -ge {SETTLED_SECS} ]; then i=0; else i=$((i + 1)); fi; done; \
         printf '\\r\\033[K'; echo \"moor: agent exited $c\"",
        quote(&start(&exit_file, script)),
        quote(&attach(&exit_file)),
    ))
}

/// Time allowed for ending the agent's session on the Box.
const STOP_DEADLINE: Duration = Duration::from_secs(30);

/// Ends the agent's tmux session on the Box, which hangs the agent up; a session already gone
/// is fine.
pub(in crate::adapters::box_agent) fn stop(
    driver: &dyn Driver,
    s: &SessionRef,
) -> Result<(), EngineError> {
    let spec = ExecSpec {
        argv: vec![
            "sh".to_owned(),
            "-c".to_owned(),
            format!("! command -v tmux >/dev/null 2>&1 || tmux kill-session -t {SESSION} 2>/dev/null || true"),
        ],
        env: Vec::new(),
        cwd: None,
        output: None,
    };
    let failed = |message: String| EngineError::Failed {
        step: "stop",
        message,
    };
    match driver.exec(&s.handle, &spec, Deadline(STOP_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => Ok(()),
        Ok(ExecOutcome::Exited { code }) => Err(failed(format!("exited {code}"))),
        Ok(ExecOutcome::Interrupted) => Err(failed("interrupted".to_owned())),
        Err(e) => Err(failed(e.to_string())),
    }
}

#[cfg(test)]
#[path = "tests/detach.rs"]
mod tests;
