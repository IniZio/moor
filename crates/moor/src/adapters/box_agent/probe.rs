//! One probe: herdr-core's view of the agent's terminal, the hooks' note and, while the agent is
//! not working, its screen, all in one Box command.

use moor_core::activity::{BlockKind, Life, Note, Probe, Status, Term, WaitCheck};
use moor_core::seam::agent_adapter::AgentAdapter;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{BoxStep, Driver, StepDone};
use moor_core::seam::engine::SessionRef;
use moor_core::seam::proc::Deadline;
use serde_json::Value;

use super::launch::holder;
use super::launch::AGENT_PATH_LINE;

/// Reads the agent once within `deadline`, undecided. A turn the agent reports working is real
/// use of the Box, so it touches the Box's lease; any other read only looks.
pub(super) fn probe(
    driver: &dyn Driver,
    profile: &dyn AgentAdapter,
    s: &SessionRef,
    deadline: Deadline,
) -> Probe {
    let steps = [BoxStep::Run(script())];
    let Some(Ok(StepDone::Ran { stdout, .. })) =
        driver.batch(&s.handle, &[], &steps, deadline).pop()
    else {
        return Probe::TimedOut;
    };
    let probe = parse(profile, &stdout);
    if matches!(
        probe.term().and_then(|t| t.status.as_ref()),
        Some(Status::Working { .. })
    ) {
        let _ = driver.touch(&s.handle);
    }
    probe
}

/// The Box command: the terminal on the first line (`null` when there is none), the note on the
/// second, then the screen unless the agent is working.
pub(super) fn script() -> String {
    let p = holder::PROGRAM;
    let s = holder::SESSION;
    let doc_get =
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"doc.get","params":{{"name":"{s}"}}}}"#);
    format!(
        "{AGENT_PATH_LINE}\n\
         t=$({p} get {s} --json 2>/dev/null) || t=null\n\
         printf '%s\\n' \"$t\"\n\
         printf '%s\\n' '{doc_get}' | {p} connect 2>/dev/null | head -n 1 | grep . || echo null\n\
         case $t in *'\"state\":\"working\"'*) ;; null) ;; *) {p} read {s} 2>/dev/null ;; esac\n\
         exit 0"
    )
}

/// The probe the Box command printed.
pub(super) fn parse(profile: &dyn AgentAdapter, out: &str) -> Probe {
    let mut lines = out.splitn(3, '\n');
    let (Some(term), Some(doc)) = (lines.next(), lines.next()) else {
        return Probe::TimedOut;
    };
    let Ok(term) = serde_json::from_str::<Value>(term) else {
        return Probe::TimedOut;
    };
    let doc = serde_json::from_str::<Value>(doc).unwrap_or(Value::Null);
    let note = doc
        .pointer("/result/value")
        .map_or_else(Note::default, |v| profile.parse_note(v));
    let screen = lines.next().filter(|s| !s.is_empty()).map(str::to_owned);
    Probe::Read {
        term: parse_term(&term),
        note,
        screen,
    }
}

/// The terminal `hc get --json` describes; none for `null` or anything else.
fn parse_term(v: &Value) -> Option<Term> {
    let life = match v.get("state")?.as_str()? {
        "live" => Life::Live,
        "finished" => Life::Finished {
            exit: v
                .get("exit_code")
                .and_then(Value::as_i64)
                .and_then(|c| i32::try_from(c).ok()),
        },
        _ => return None,
    };
    Some(Term {
        life,
        status: v.get("status").and_then(parse_status),
        status_at: v
            .get("status_ms")
            .and_then(Value::as_i64)
            .map(Ts::from_millis),
        turn: v.get("turn").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// The status an agent reported with OSC 7501.
fn parse_status(v: &Value) -> Option<Status> {
    let text = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
    };
    Some(match v.get("state")?.as_str()? {
        "working" => Status::Working { doing: text("msg") },
        "idle" | "done" => Status::Idle,
        "blocked" => Status::Blocked {
            kind: match v.get("kind").and_then(Value::as_str) {
                Some("permission") => Some(BlockKind::Permission),
                Some("question") => Some(BlockKind::Question),
                _ => None,
            },
            msg: text("msg"),
        },
        "error" => Status::Error { msg: text("msg") },
        _ => return None,
    })
}

/// Runs the agent's wait check in the Box; anything but a plain answer is unknown.
pub(super) fn check_wait(
    driver: &dyn Driver,
    profile: &dyn AgentAdapter,
    s: &SessionRef,
    deadline: Deadline,
) -> WaitCheck {
    let Some(script) = profile.wait_check_script() else {
        return WaitCheck::Unknown;
    };
    let steps = [BoxStep::Run(script)];
    match driver.batch(&s.handle, &[], &steps, deadline).pop() {
        Some(Ok(StepDone::Ran { stdout, .. })) => match stdout.trim() {
            "none" => WaitCheck::Nothing,
            "running" => WaitCheck::Running,
            _ => WaitCheck::Unknown,
        },
        _ => WaitCheck::Unknown,
    }
}

#[cfg(test)]
mod tests;
