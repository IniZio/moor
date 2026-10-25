//! The agent runs in a detached tmux session on the Box and the host pane attaches to it, so a
//! dropped ssh link leaves the agent running and a new pane can attach to it.

use std::time::Duration;

use moor_core::boxscript::{command_line, quote};
use moor_core::name::BoxName;
use moor_core::name::{BoxPath, NameError};
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::{AgentFate, EngineError, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneCmd, PaneRef, Surface, WorkspaceRef};

/// The tmux session the agent runs in; a Box runs one agent at a time.
pub(in crate::adapters::box_agent) const SESSION: &str = "moor-agent";
/// Where the agent's exit status lands, relative to the Box workdir.
const EXIT_FILE: &str = ".moor/agent.exit";
/// The Box command that attaches to the agent's session and exits with the agent's status once
/// the session is over.
fn attach(exit_file: &str) -> String {
    format!(
        "if tmux has-session -t {SESSION} 2>/dev/null; then tmux attach-session -t {SESSION}; fi; \
         {}",
        exited(exit_file)
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

/// The Box command that exits with the agent's recorded status, or 1 when it left none.
fn exited(exit_file: &str) -> String {
    format!("if [ -s {exit_file} ]; then exit \"$(cat {exit_file})\"; fi; exit 1")
}

/// The host pane's shell line: run `box_cmd` on the Box through `connect`, then print the exit
/// marker with its status. moor asks the Box whether the agent outlived a pane that ended.
fn line(connect: &[String], box_cmd: &str) -> String {
    format!(
        "{} {}; c=$?; printf '\\r\\033[K'; echo \"moor: agent exited $c\"",
        command_line(connect),
        quote(box_cmd)
    )
}

/// The pane line that starts the agent script at `script` on the Box. The script travels as a
/// file, not inside this line: each shell layer between here and tmux would quote it again.
pub(super) fn pane_line(
    connect: &[String],
    workdir: &str,
    script: &str,
) -> Result<String, NameError> {
    let exit_file = quote(BoxPath::under(workdir, EXIT_FILE)?.as_str());
    Ok(line(connect, &start(&exit_file, script)))
}

/// Opens a pane in `ws` that attaches to the running agent of `s`.
pub(in crate::adapters::box_agent) fn reattach(
    driver: &dyn Driver,
    surface: &dyn Surface,
    s: &SessionRef,
    ws: &WorkspaceRef,
    name: &BoxName,
) -> Result<PaneRef, EngineError> {
    let failed = |message: String| EngineError::Failed {
        step: "reattach",
        message,
    };
    let connect = driver
        .connect_argv(&s.handle)
        .map_err(|e| failed(e.to_string()))?;
    let exit_file = quote(
        BoxPath::under(s.workdir.as_str(), EXIT_FILE)
            .map_err(|e| failed(e.to_string()))?
            .as_str(),
    );
    surface
        .open_pane(
            ws,
            &PaneCmd {
                label: format!("agent:{}", name.as_str()),
                argv: vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    line(&connect, &attach(&exit_file)),
                ],
                env: Vec::new(),
            },
        )
        .map_err(|e| failed(e.to_string()))
}

/// Time allowed for each Box command that tells what became of the agent.
const FATE_DEADLINE: Duration = Duration::from_secs(20);

/// Asks the Box whether the agent's session still runs and, once it is over, for its status.
pub(in crate::adapters::box_agent) fn fate(
    driver: &dyn Driver,
    s: &SessionRef,
    deadline: Deadline,
) -> Result<AgentFate, EngineError> {
    let deadline = Deadline(deadline.0.min(FATE_DEADLINE));
    let run = |cmd: String| {
        let spec = ExecSpec {
            argv: vec!["sh".to_owned(), "-c".to_owned(), cmd],
            env: Vec::new(),
            cwd: Some(s.workdir.clone()),
            output: None,
        };
        match driver.exec(&s.handle, &spec, deadline) {
            Ok(ExecOutcome::Exited { code }) => Ok(code),
            Ok(ExecOutcome::Interrupted) => Err(EngineError::Failed {
                step: "fate",
                message: "interrupted".to_owned(),
            }),
            Err(e) => Err(EngineError::Box(e)),
        }
    };
    fate_by(run)
}

/// What became of the agent, told by Box commands that `run` runs in the workdir.
fn fate_by(run: impl Fn(String) -> Result<i32, EngineError>) -> Result<AgentFate, EngineError> {
    if run(format!("tmux has-session -t {SESSION} 2>/dev/null"))? == 0 {
        return Ok(AgentFate::Running);
    }
    let exit = run(exited(EXIT_FILE))?;
    Ok(AgentFate::Ended { exit: Some(exit) })
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
