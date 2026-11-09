//! A sleep or suspend whose Box got new hands while it ran records nothing and warns once.

use super::*;

/// Gives Box `id` new hands while the Driver sleeps a machine, the first time only.
fn replaced_while_sleeping(w: &World, id: BoxId) {
    let store = w.cx.store().unwrap().clone();
    let once = std::sync::Once::new();
    w.driver
        .on_sleep(move || once.call_once(|| store.replace_hands(id)));
}

fn warnings(w: &World) -> Vec<String> {
    w.cx.take_warnings().into_iter().map(|x| x.code).collect()
}

#[test]
fn an_auto_sleep_of_replaced_hands_records_nothing_and_warns() {
    let (w, _) = live_world(32_768);
    let b = started(&w, "b");
    idle(&w, b);
    holds(&w, b, CLEAN);
    replaced_while_sleeping(&w, b);
    w.cx.clock().sleep(std::time::Duration::from_mins(6));
    drop(warnings(&w));

    autosleep::run(&w.cx, w.cx.store().unwrap(), b, autosleep::AgentRead::Plain);

    assert_eq!(lifecycle(&w, b), Ready);
    assert_eq!(warnings(&w), ["stale_hands_write"]);
}

#[test]
fn a_suspend_for_room_of_replaced_hands_records_nothing_and_warns() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    guest_memory(&w, b, CEILING);
    idle(&w, b);
    let (w, _) = agents_report(w, &[b], &[a]);
    holds(&w, b, CLEAN);
    replaced_while_sleeping(&w, b);
    drop(warnings(&w));

    tick(&w, hv, BELOW_RESERVE, 0, 0);

    assert_eq!(lifecycle(&w, b), Ready);
    assert!(warnings(&w).contains(&"stale_hands_write".to_owned()));
}

#[test]
fn a_last_resort_suspend_of_replaced_hands_records_nothing_and_warns() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    guest_memory(&w, b, CEILING);
    let (w, _) = agents_report(w, &[], &[a, b]);
    holds(&w, b, CLEAN);
    replaced_while_sleeping(&w, b);
    drop(warnings(&w));

    tick(&w, hv, BELOW_RESERVE, 0, 0);

    assert_eq!(lifecycle(&w, b), Ready);
    assert!(warnings(&w).contains(&"stale_hands_write".to_owned()));
}
