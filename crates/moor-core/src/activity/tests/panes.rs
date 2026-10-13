//! Pane and multiplexer readings.

use super::{asking, exited, fresh, obs, waiting, Case, SILENT, TASKS};
use crate::activity::Source as Src;
use crate::activity::{AgentState as S, HookKind as K, HookRead, PaneRead, PaneStatus as P};

#[test]
fn absent_hook_uses_herdr_status() {
    let a = || HookRead::Absent;
    Case::new("working", a(), obs(P::Working, "")).expect(S::Working, Src::Herdr);
    Case::new("idle is reported", a(), obs(P::Idle, "")).expect(S::Idle, Src::Herdr);
    Case::new(
        "blocked shows the profile's prompt line, not the footer",
        a(),
        asking(P::Blocked, "Allow edit?\n\nesc to interrupt", "Allow edit?"),
    )
    .expect(waiting("Allow edit?"), Src::Herdr);
    Case::new(
        "blocked without a prompt line shows nothing",
        a(),
        obs(P::Blocked, "out\nesc to interrupt"),
    )
    .expect(SILENT, Src::Herdr);
    Case::new(
        "blank prompt line shows nothing",
        a(),
        asking(P::Blocked, "", "  "),
    )
    .expect(SILENT, Src::Herdr);
}

#[test]
fn absent_hook_reads_the_pane_text() {
    let a = || HookRead::Absent;
    Case::new("idle with tasks", a(), obs(P::Idle, TASKS)).expect(S::Working, Src::Pane);
    Case::new("unknown with tasks", a(), obs(P::Unknown, TASKS)).expect(S::Working, Src::Pane);
    Case::new("unknown without", a(), obs(P::Unknown, "hello")).expect(S::Unknown, Src::Pane);
    Case::new("no pane", a(), PaneRead::Absent).expect(S::Unknown, Src::Pane);
    Case::new("pane timed out", a(), PaneRead::TimedOut).expect(S::Unknown, Src::Timeout);
}

#[test]
fn ended_pane_wins() {
    Case::new("exit over busy hook", fresh(K::Busy), exited(3))
        .expect(S::Exited { code: 3 }, Src::Pane);
    Case::new("exit while young", HookRead::Absent, exited(0))
        .started(1)
        .expect(S::Exited { code: 0 }, Src::Pane);
    Case::new("gone over busy hook", fresh(K::Busy), PaneRead::Gone).expect(S::Idle, Src::Pane);
}
