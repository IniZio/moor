use super::*;

/// The next report from `hv`, ten seconds after the last, with PSI `some` and `full`.
fn tick(w: &World, hv: i64, available_mib: u64, some: u32, full: u32) {
    w.cx.clock().sleep(std::time::Duration::from_secs(10));
    report_psi(w, hv, available_mib, Some(some), Some(full));
}

/// Makes the agents of `idle` report Idle and those of `busy` Busy; the engine, to read steers.
fn agents_report(
    mut w: World,
    idle: &[BoxId],
    busy: &[BoxId],
) -> (World, std::sync::Arc<crate::testing::engine::FakeEngine>) {
    use moor_core::activity::{HookKind, HookRead, HookRecord, PaneRead, Probes};
    let probes = |kind: HookKind| Probes {
        hook: HookRead::Fresh(HookRecord {
            kind,
            session_id: None,
            message: None,
            options: Vec::new(),
            doing: None,
            at: w.cx.clock().now(),
            turn: None,
        }),
        pane: PaneRead::Absent,
    };
    let mut engine = crate::testing::engine::FakeEngine::new();
    for id in idle {
        engine = engine.probes(&format!("moor-{}", id.get()), probes(HookKind::Idle));
    }
    for id in busy {
        engine = engine.probes(&format!("moor-{}", id.get()), probes(HookKind::Busy));
    }
    let engine = std::sync::Arc::new(engine);
    w.cx = w.cx.with_engine(engine.clone());
    (w, engine)
}

fn notes(engine: &crate::testing::engine::FakeEngine) -> Vec<String> {
    engine
        .steers()
        .into_iter()
        .filter_map(|(_, s)| match s {
            moor_core::seam::engine::Steer::Text(t) if t.starts_with("This Box was suspended") => {
                Some(t)
            }
            _ => None,
        })
        .collect()
}

fn wake_up(w: &World, id: BoxId) -> Result<(), crate::error::CommandError> {
    let box_name = w.cx.store().unwrap().box_row(id).unwrap().unwrap().name;
    crate::ops::wake::run(&w.cx, &moor_core::BoxReq { box_name }).map(|_| ())
}

#[test]
fn sustained_pressure_suspends_exactly_the_longest_idle_box_on_the_third_report() {
    use moor_core::domain::box_state::BoxLifecycle;
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    idle(&w, a);
    idle(&w, b);
    let (w, _) = agents_report(w, &[a, b], &[]);
    tick(&w, hv, 32_768, 1000, 0);
    tick(&w, hv, 32_768, 1000, 0);
    assert_eq!(
        lifecycle(&w, a),
        BoxLifecycle::Ready,
        "two reports are not sustained"
    );
    tick(&w, hv, 32_768, 1000, 0);
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Asleep);
    assert_eq!(lifecycle(&w, b), BoxLifecycle::Ready);
    assert_eq!(
        asleep_reason(&w, a).as_deref(),
        Some("memory pressure on hv")
    );
}

#[test]
fn a_pressure_suspension_holds_the_next_one_off_for_thirty_seconds() {
    use moor_core::domain::box_state::BoxLifecycle;
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    idle(&w, a);
    idle(&w, b);
    let (w, _) = agents_report(w, &[a, b], &[]);
    for _ in 0..3 {
        tick(&w, hv, 32_768, 1000, 0);
    }
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Asleep);
    tick(&w, hv, 32_768, 1000, 0);
    tick(&w, hv, 32_768, 1000, 0);
    assert_eq!(
        lifecycle(&w, b),
        BoxLifecycle::Ready,
        "20 s after the suspension"
    );
    tick(&w, hv, 32_768, 1000, 0);
    assert_eq!(
        lifecycle(&w, b),
        BoxLifecycle::Asleep,
        "30 s after the suspension"
    );
}

#[test]
fn sustained_pressure_never_suspends_a_working_box() {
    use moor_core::domain::box_state::BoxLifecycle;
    let (w, hv) = live_world(32_768);
    let working = started(&w, "working");
    let (w, _) = agents_report(w, &[], &[working]);
    for _ in 0..4 {
        tick(&w, hv, 32_768, 2000, 0);
    }
    assert_eq!(lifecycle(&w, working), BoxLifecycle::Ready);
}

fn assert_told_of_the_critical_suspension(sent: &[String]) {
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent.first().is_some_and(|n| n
            .starts_with("This Box was suspended for 10s because host memory was critical.")
            && n.ends_with("Re-run anything that failed.")),
        "{sent:?}"
    );
}

#[test]
fn critical_memory_suspends_the_newest_working_box_which_resumes_first_and_is_told_once() {
    use moor_core::domain::box_state::BoxLifecycle;
    let (w, hv) = live_world(32_768);
    let old = started(&w, "old");
    put_to_sleep(&w, old);
    let (b, c) = (started(&w, "b"), started(&w, "c"));
    let (w, engine) = agents_report(w, &[], &[b, c]);
    tick(&w, hv, SHORT, 0, 0);
    assert!(wake_up(&w, old).is_err());
    assert_eq!(lifecycle(&w, old), BoxLifecycle::Pending);

    tick(&w, hv, 512, 0, 0);
    assert_eq!(lifecycle(&w, c), BoxLifecycle::Pending);
    assert_eq!(lifecycle(&w, b), BoxLifecycle::Ready);
    assert_eq!(
        asleep_reason(&w, c).as_deref(),
        Some("last-resort suspend: memory critical on hv")
    );
    assert!(notes(&engine).is_empty());

    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(
        lifecycle(&w, c),
        BoxLifecycle::Ready,
        "resumes before the older waiter"
    );
    assert_eq!(lifecycle(&w, old), BoxLifecycle::Pending);
    assert_told_of_the_critical_suspension(&notes(&engine));

    put_to_sleep(&w, c);
    wake_up(&w, c).unwrap();
    assert_eq!(
        notes(&engine).len(),
        1,
        "told only after the last-resort suspension"
    );
}

#[test]
fn the_node_list_shows_sustained_pressure_while_it_lasts() {
    let (w, hv) = live_world(32_768);
    let since = |w: &World| {
        let list = nodes::list(&w.cx).unwrap();
        list.nodes
            .iter()
            .find(|n| n.name == "hv")
            .unwrap()
            .pressure_since
    };
    tick(&w, hv, 32_768, 1000, 0);
    tick(&w, hv, 32_768, 1000, 0);
    assert_eq!(since(&w), None);
    tick(&w, hv, 32_768, 1000, 0);
    assert_eq!(since(&w), Some(w.cx.clock().now().as_millis() / 1000));
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(since(&w), None);
}

#[test]
fn a_box_that_goes_idle_while_a_start_waits_is_suspended_on_the_next_report_and_the_waiter_starts()
{
    use moor_core::domain::box_state::BoxLifecycle;
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    report(&w, hv, SHORT, None);
    let (c, _) = waits(&w, "c");
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Ready, "nothing idle yet");
    let (w, _) = agents_report(w, &[a], &[b]);
    idle(&w, a);
    tick(&w, hv, SHORT, 0, 0);
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Asleep);
    assert_eq!(lifecycle(&w, b), BoxLifecycle::Ready);
    assert_eq!(asleep_reason(&w, a).as_deref(), Some("made room for c"));
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(lifecycle(&w, c), BoxLifecycle::Ready);
}

#[test]
fn a_short_node_with_nothing_waiting_keeps_its_idle_boxes_up() {
    use moor_core::domain::box_state::BoxLifecycle;
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    idle(&w, a);
    idle(&w, b);
    let (w, _) = agents_report(w, &[a, b], &[]);
    for _ in 0..4 {
        tick(&w, hv, SHORT, 0, 0);
    }
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Ready);
    assert_eq!(lifecycle(&w, b), BoxLifecycle::Ready);
}
