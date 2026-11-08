//! Agent reads for tests: what herdr-core and the hooks' note show of an agent.

use moor_core::activity::{BlockKind, Life, Note, Probe, Question, Status, Term};
use moor_core::seam::clock::Ts;

/// A live agent reporting `status`, its status set at `at`, after `turn` ended turns.
pub(crate) fn read(status: Option<Status>, at: Ts, turn: u64) -> Probe {
    Probe::Read {
        term: Some(Term {
            life: Life::Live,
            status,
            status_at: Some(at),
            turn,
        }),
        note: Note::default(),
        screen: None,
    }
}

/// A working agent.
pub(crate) fn working(at: Ts) -> Probe {
    read(Some(Status::Working { doing: None }), at, 0)
}

/// A working agent showing `doing`.
pub(crate) fn doing(text: &str, at: Ts) -> Probe {
    read(
        Some(Status::Working {
            doing: Some(text.to_owned()),
        }),
        at,
        0,
    )
}

/// An idle agent.
pub(crate) fn idle(at: Ts) -> Probe {
    read(Some(Status::Idle), at, 1)
}

/// An idle agent whose hooks left `said` as its last reply.
pub(crate) fn said(text: &str, at: Ts) -> Probe {
    with_note(
        idle(at),
        Note {
            said: Some(text.to_owned()),
            ..Note::default()
        },
    )
}

/// An agent asking `question` with `options`, from its question tool.
pub(crate) fn asking(question: &str, options: &[&str], at: Ts) -> Probe {
    with_note(
        read(
            Some(Status::Blocked {
                kind: Some(BlockKind::Question),
                msg: Some(format!("answer: {question}")),
            }),
            at,
            1,
        ),
        Note {
            question: Some(Question {
                text: question.to_owned(),
                options: options.iter().map(|o| (*o).to_owned()).collect(),
            }),
            ..Note::default()
        },
    )
}

/// An agent asking permission with `msg`.
pub(crate) fn permission(msg: &str, at: Ts) -> Probe {
    read(
        Some(Status::Blocked {
            kind: Some(BlockKind::Permission),
            msg: Some(msg.to_owned()),
        }),
        at,
        1,
    )
}

/// An idle agent that left `on` running in the background, all of it local when `local`.
pub(crate) fn waiting(on: &[&str], local: bool, at: Ts) -> Probe {
    with_note(
        idle(at),
        Note {
            background: on.iter().map(|o| (*o).to_owned()).collect(),
            background_local: local,
            ..Note::default()
        },
    )
}

/// An agent that ended with `exit`.
pub(crate) fn exited(exit: Option<i32>) -> Probe {
    Probe::Read {
        term: Some(Term {
            life: Life::Finished { exit },
            status: None,
            status_at: None,
            turn: 0,
        }),
        note: Note::default(),
        screen: None,
    }
}

/// A Box with no agent terminal.
pub(crate) fn gone() -> Probe {
    Probe::Read {
        term: None,
        note: Note::default(),
        screen: None,
    }
}

/// `p` with its ended-turn count set to `turn`.
pub(crate) fn at_turn(mut p: Probe, turn: u64) -> Probe {
    if let Probe::Read { term: Some(t), .. } = &mut p {
        t.turn = turn;
    }
    p
}

/// `p` showing `text` on screen.
pub(crate) fn on_screen(mut p: Probe, text: &str) -> Probe {
    if let Probe::Read { screen, .. } = &mut p {
        *screen = Some(text.to_owned());
    }
    p
}

/// `p` with the hooks' note `n`.
pub(crate) fn with_note(mut p: Probe, n: Note) -> Probe {
    if let Probe::Read { note, .. } = &mut p {
        *note = n;
    }
    p
}

/// `p` whose note names the agent session `id`.
pub(crate) fn session(mut p: Probe, id: &str) -> Probe {
    if let Probe::Read { note, .. } = &mut p {
        note.session_id = Some(id.to_owned());
    }
    p
}
