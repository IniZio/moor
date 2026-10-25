//! Hook reading, cache and startup rules.

use super::{asking, fresh, obs, said, waiting, Case, BACKGROUND, OLD, SILENT};
use crate::activity::Source as Src;
use crate::activity::{
    AgentState as S, HookKind as K, HookRead, HookRecord, PaneRead, PaneStatus as P,
};

#[test]
fn fresh_hook_states() {
    let none = || PaneRead::Absent;
    Case::new("busy", fresh(K::Busy), none()).expect(S::Working, Src::Hooks);
    Case::new("idle", fresh(K::Idle), none()).expect(S::Idle, Src::Hooks);
    Case::new("blocked", HookRead::Fresh(said("Proceed?")), none())
        .expect(waiting("Proceed?"), Src::Hooks);
    Case::new("beats the cache", fresh(K::Busy), none())
        .cache(K::Idle, 5)
        .expect(S::Working, Src::Hooks);
    Case::new("beats herdr", fresh(K::Idle), obs(P::Working, "")).expect(S::Idle, Src::Hooks);
    Case::new("idle notice after idle", fresh(K::Idle), none())
        .cache(K::Idle, 30)
        .expect(S::Idle, Src::Hooks);
}

#[test]
fn background_override_idle_hooks() {
    let idle = || obs(P::Idle, BACKGROUND);
    Case::new("fresh idle", fresh(K::Idle), idle()).expect(S::Working, Src::Pane);
    Case::new("busy stays", fresh(K::Busy), idle()).expect(S::Working, Src::Hooks);
    Case::new("cached idle", HookRead::TimedOut, idle())
        .cache(K::Idle, 10)
        .expect(S::Working, Src::Pane);
    Case::new(
        "zero",
        fresh(K::Idle),
        obs(P::Idle, "0 background tasks running"),
    )
    .expect(S::Idle, Src::Hooks);
    Case::new(
        "one",
        fresh(K::Idle),
        obs(P::Idle, "ok\n1 background task running\n"),
    )
    .expect(S::Working, Src::Pane);
}

#[test]
fn background_markers_in_real_wordings() {
    for text in [
        "2 Background Agents running",
        "1 background task",
        "Waiting for 3 background agents",
    ] {
        Case::new(text, fresh(K::Idle), obs(P::Idle, text)).expect(S::Working, Src::Pane);
    }
    let old = format!("{BACKGROUND}\n1\n2\n3\n4\n5\n6\n");
    Case::new("old scrollback", fresh(K::Idle), obs(P::Idle, &old)).expect(S::Idle, Src::Hooks);
    let near = format!("{BACKGROUND}\n\n2\n3\n4\n5\n6\n");
    Case::new("within six lines", fresh(K::Idle), obs(P::Idle, &near))
        .expect(S::Working, Src::Pane);
}

#[test]
fn timed_out_read_uses_fresh_cache_only() {
    let none = || PaneRead::Absent;
    let t = || HookRead::TimedOut;
    Case::new("busy", t(), none())
        .cache(K::Busy, 119)
        .expect(S::Working, Src::HooksCached);
    Case::new("idle", t(), none())
        .cache(K::Idle, 10)
        .expect(S::Idle, Src::HooksCached);
    Case::new("blocked", t(), none())
        .cache_record(said("ok?"), 10)
        .expect(waiting("ok?"), Src::HooksCached);
    Case::new("stale", t(), none())
        .cache(K::Busy, 120)
        .expect(S::Unknown, Src::Timeout);
    Case::new("no cache", t(), none()).expect(S::Unknown, Src::Timeout);
}

#[test]
fn timed_out_read_never_reaches_the_pane() {
    let t = || HookRead::TimedOut;
    Case::new("idle pane", t(), obs(P::Idle, "")).expect(S::Unknown, Src::Timeout);
    Case::new("working pane", t(), obs(P::Working, ""))
        .cache(K::Idle, 500)
        .expect(S::Unknown, Src::Timeout);
}

#[test]
fn absent_hook_prefers_fresh_cache() {
    let a = || HookRead::Absent;
    Case::new("fresh cache", a(), obs(P::Idle, ""))
        .cache(K::Busy, 60)
        .expect(S::Working, Src::HooksCached);
    Case::new("stale cache", a(), obs(P::Working, ""))
        .cache(K::Idle, 600)
        .expect(S::Working, Src::Herdr);
}

#[test]
fn starting_holds_while_young() {
    let a = || HookRead::Absent;
    Case::new("no hook", a(), obs(P::Idle, ""))
        .started(119)
        .expect(S::Starting, Src::Hooks);
    Case::new("grace over", a(), obs(P::Idle, ""))
        .started(120)
        .expect(S::Idle, Src::Herdr);
    Case::new(
        "starting hook",
        fresh(K::Starting { launch: None }),
        PaneRead::Absent,
    )
    .started(30)
    .expect(S::Working, Src::Hooks);
    Case::new(
        "starting hook, grace over",
        fresh(K::Starting { launch: None }),
        obs(P::Idle, ""),
    )
    .started(OLD)
    .expect(S::Idle, Src::Herdr);
    Case::new("busy hook", fresh(K::Busy), PaneRead::Absent)
        .started(5)
        .expect(S::Working, Src::Hooks);
}

#[test]
fn hook_message_wins_over_the_pane_text() {
    let pane = || asking(P::Blocked, "out\nesc to interrupt", "Allow rm?");
    Case::new("fresh", HookRead::Fresh(said("Allow edit?")), pane())
        .expect(waiting("Allow edit?"), Src::Hooks);
    Case::new("cached", HookRead::TimedOut, pane())
        .cache_record(said("Allow edit?"), 10)
        .expect(waiting("Allow edit?"), Src::HooksCached);
}

#[test]
fn blocked_hook_without_a_usable_message_falls_back_to_the_pane_prompt() {
    let blank = |m: Option<&str>| {
        HookRead::Fresh(HookRecord {
            message: m.map(str::to_owned),
            options: Vec::new(),
            doing: None,
            ..said("")
        })
    };
    let none = PaneRead::Absent;
    Case::new("no message", blank(None), none).expect(SILENT, Src::Hooks);
    Case::new("no prompt", blank(None), obs(P::Blocked, "x")).expect(SILENT, Src::Hooks);
    Case::new("blank message", blank(Some(" \n")), obs(P::Blocked, "x")).expect(SILENT, Src::Hooks);
    let ask = || asking(P::Blocked, "", "Pick one?");
    Case::new("no message, prompt", blank(None), ask()).expect(waiting("Pick one?"), Src::Hooks);
    Case::new("blank message, prompt", blank(Some(" ")), ask())
        .expect(waiting("Pick one?"), Src::Hooks);
}
