//! The `agent_doing` event a busy hook's line writes, at most one per Box in thirty seconds.

use std::time::Duration;

use moor_core::seam::clock::Clock;

use super::*;
use crate::testing::clock::FakeClock;

fn doing(text: &str) -> Probes {
    let mut probes = hook(HookKind::Busy);
    if let HookRead::Fresh(rec) = &mut probes.hook {
        rec.doing = Some(text.to_owned());
    }
    probes
}

fn lines(cx: &Cx, id: BoxId) -> Vec<String> {
    let events = cx.store().unwrap().events(id, 100).unwrap();
    events
        .iter()
        .filter(|e| e.kind == "agent_doing")
        .map(|e| {
            e.payload
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn read_at(cx: Cx, id: BoxId, clock: &FakeClock, probes: Probes) -> Cx {
    let cx = cx.with_engine(Arc::new(FakeEngine::new().probes(SESSION, probes)));
    read(
        &cx,
        cx.store().unwrap(),
        &row(&cx, id),
        clock.now(),
        PROBE_DEADLINE,
    )
    .unwrap();
    cx
}

#[test]
fn a_changed_line_is_written_at_most_once_in_thirty_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let clock = Arc::new(FakeClock::new(NOW));
    let (cx, _) = cx(dir.path(), &engine);
    let cx = cx.with_clock(clock.clone());
    let id = working_box(&cx, true);

    let cx = read_at(cx, id, &clock, doing("Bash: cargo test"));
    let cx = read_at(cx, id, &clock, doing("Bash: cargo test"));
    assert_eq!(
        lines(&cx, id),
        ["Bash: cargo test"],
        "an unchanged line is written once"
    );

    clock.sleep(Duration::from_secs(10));
    let cx = read_at(cx, id, &clock, doing("Edit: main.rs"));
    assert_eq!(
        lines(&cx, id),
        ["Bash: cargo test"],
        "a change within 30 s waits"
    );

    clock.sleep(Duration::from_secs(21));
    let cx = read_at(cx, id, &clock, doing("Edit: main.rs"));
    assert_eq!(lines(&cx, id), ["Bash: cargo test", "Edit: main.rs"]);

    clock.sleep(Duration::from_secs(31));
    let cx = read_at(cx, id, &clock, doing("Edit: main.rs"));
    assert_eq!(
        lines(&cx, id),
        ["Bash: cargo test", "Edit: main.rs"],
        "an unchanged line is not written again however late"
    );
}

#[test]
fn a_busy_hook_without_a_line_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, hook(HookKind::Busy)));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    run_once(&cx, id);
    assert!(lines(&cx, id).is_empty());
}
