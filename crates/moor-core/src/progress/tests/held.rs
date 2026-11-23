//! The line of a quiet Box that auto-close left open.

use super::*;

fn held(id: i64, why: &str) -> EventView {
    ev(
        id,
        "auto_close_held",
        "2026-11-18T09:00:00Z",
        json!({"due": "idle", "why": why}),
    )
}

#[test]
fn an_asleep_box_held_open_says_why_until_it_is_used_again() {
    let b = row(BoxLifecycle::Asleep, SessionState::Idle);
    let why = "it holds 1 unpushed commit(s) and 0 uncommitted file(s)";
    let t = derive(&b, &[held(1, why)], ten());
    assert_eq!(t.held.as_deref(), Some(why));
    assert_eq!(t.line, format!("asleep · not closed: {why}"));
    let used = [
        held(1, why),
        ev(2, "session_started", "2026-11-18T09:30:00Z", json!({})),
    ];
    let t = derive(&b, &used, ten());
    assert_eq!((t.held, t.line.as_str()), (None, "asleep"));
}

#[test]
fn a_done_box_held_open_says_why_after_its_own_line() {
    let b = row(BoxLifecycle::Ready, SessionState::Idle);
    let said = ev(
        1,
        "agent_said",
        "2026-11-18T08:00:00Z",
        json!({"text": "finished"}),
    );
    let t = derive(
        &b,
        &[
            said,
            held(2, "it holds 2 unpushed commit(s) and 0 uncommitted file(s)"),
        ],
        ten(),
    );
    assert_eq!(
        t.line,
        "finished · not closed: it holds 2 unpushed commit(s) and 0 uncommitted file(s)"
    );
}
