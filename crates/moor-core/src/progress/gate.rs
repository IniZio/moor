//! What a Box's gates add to its state and its line.

use super::{ask_id, NeedKind, Needs, Progress};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::gate::{GateNext, GateStatus, Wait, WaitKind};
use crate::view::BoxView;

/// The status of the gates of an open Box; none for a Box without gates or a closed one.
fn open_status(b: &BoxView) -> Option<&GateStatus> {
    b.gate
        .as_ref()
        .map(|g| &g.status)
        .filter(|_| matches!(b.lifecycle, BoxLifecycle::Ready | BoxLifecycle::Asleep))
}

/// The name of the prompt that asks about `wait`. It carries the number of gate lines written when
/// the wait began, so an answer to an earlier wait never lands on a later one.
#[must_use]
pub fn gate_ask_id(wait: &Wait) -> String {
    ask_id(Some(&wait.gate), Some(&wait.seq.to_string()), &[])
}

/// The card of the gate that waits for a person in `b`, an open Box.
pub(super) fn needs(b: &BoxView) -> Option<Needs> {
    open_status(b)?;
    let wait = b.gate.as_ref()?.wait.as_ref()?;
    let (kind, detail) = match wait.kind {
        WaitKind::Approve => (NeedKind::Approve, wait.note.clone()),
        WaitKind::Stuck => (
            NeedKind::Stuck,
            wait.command.clone().or_else(|| {
                (!wait.missing.is_empty()).then(|| format!("missing: {}", wait.missing.join(", ")))
            }),
        ),
    };
    Some(Needs {
        id: gate_ask_id(wait),
        kind,
        question: wait.question(),
        detail,
        picks: Vec::new(),
        other: None,
        approve: None,
        plan: false,
        gate: Some(wait.clone()),
    })
}

/// Whether the agent of `b` has gone quiet: a person asked to approve before that cannot judge.
const fn quiet(b: &BoxView) -> bool {
    matches!(b.session_state, SessionState::Idle | SessionState::Ended)
}

/// The state the gates force on an open Box, even asleep: one that waits for a person needs them,
/// one that failed fails the Box. A gate that asks the owner waits until the agent is quiet.
pub(super) fn forced(b: &BoxView) -> Option<Progress> {
    match open_status(b)? {
        GateStatus::Stuck { .. } => Some(Progress::NeedsYou),
        GateStatus::Open {
            next: GateNext::Ask,
            ..
        } if quiet(b) => Some(Progress::NeedsYou),
        GateStatus::Failed { .. } => Some(Progress::Failed),
        GateStatus::Open { .. } | GateStatus::Passed => None,
    }
}

/// Whether a gate is still open, so an agent that ended its turn has not finished the Box.
pub(super) fn holds(b: &BoxView) -> bool {
    b.gate
        .as_ref()
        .is_some_and(|g| g.status != GateStatus::Passed)
}

/// What the agent wrote with its last accepted report, once every gate has passed: it says what
/// was done, where the agent's last sentence may be stale.
pub(super) fn done_note(b: &BoxView) -> Option<&str> {
    let gate = b.gate.as_ref()?;
    (gate.status == GateStatus::Passed).then_some(gate.note.as_deref())?
}

/// The line a gate gives a Box in `state`, when it has something to say that the agent's own
/// words do not.
pub(super) fn line(state: Progress, b: &BoxView) -> Option<String> {
    let status = open_status(b)?;
    match (state, status) {
        (Progress::Working, GateStatus::Open { gate, next, .. }) if quiet(b) => Some(match next {
            GateNext::Agent => format!("gate {gate}: waiting for the agent's evidence"),
            GateNext::RunChecks => format!("gate {gate}: running its checks"),
            GateNext::Ask => format!("gate {gate}: waiting for your approval"),
        }),
        (Progress::NeedsYou, GateStatus::Open { gate, .. }) => {
            Some(format!("gate {gate}: waiting for your approval"))
        }
        (Progress::NeedsYou, GateStatus::Stuck { gate, why, .. }) => {
            Some(format!("gate {gate} is stuck: {why}"))
        }
        (Progress::Failed, GateStatus::Failed { gate, why }) => {
            Some(format!("gate {gate} failed: {why}"))
        }
        _ => None,
    }
}
