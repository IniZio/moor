use moor_core::domain::box_state::BoxLifecycle;

use super::*;

/// How many events of `kind` Box `id` holds.
fn count(w: &World, id: BoxId, kind: &str) -> usize {
    w.cx.store()
        .unwrap()
        .events(id, 1000)
        .unwrap()
        .iter()
        .filter(|e| e.kind == kind)
        .count()
}

fn report_after(w: &World, hv: i64, disk: Option<(u64, u64)>) {
    w.cx.clock().sleep(std::time::Duration::from_secs(5));
    report_disk(w, hv, disk);
}

fn low_flag(w: &World) -> bool {
    nodes::list(&w.cx)
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.name == "hv")
        .unwrap()
        .disk_low
}

#[test]
fn a_node_going_low_tells_its_boxes_once() {
    let (w, hv) = live_world(32_768);
    let a = started(&w, "a");
    report_after(&w, hv, Some((40, 100)));
    assert_eq!(count(&w, a, "node_disk_low"), 0);
    assert!(!low_flag(&w));

    report_after(&w, hv, Some((15, 100)));
    assert_eq!(count(&w, a, "node_disk_low"), 1);
    assert!(low_flag(&w));

    report_after(&w, hv, Some((14, 100)));
    report_after(&w, hv, Some((22, 100)));
    assert_eq!(count(&w, a, "node_disk_low"), 1);
    assert_eq!(count(&w, a, "node_disk_recovered"), 0);
    assert!(low_flag(&w));
}

#[test]
fn a_low_node_that_climbs_past_the_margin_tells_its_boxes_it_recovered() {
    let (w, hv) = live_world(32_768);
    let a = started(&w, "a");
    report_after(&w, hv, Some((15, 100)));
    report_after(&w, hv, Some((22, 100)));
    assert_eq!(count(&w, a, "node_disk_recovered"), 0);
    report_after(&w, hv, Some((26, 100)));
    assert_eq!(count(&w, a, "node_disk_recovered"), 1);
    assert!(!low_flag(&w));
    let kept = w.cx.store().unwrap().box_row(a).unwrap().unwrap();
    assert_eq!(kept.lifecycle, BoxLifecycle::Ready);
}

#[test]
fn a_report_without_disk_figures_leaves_a_low_node_low() {
    let (w, hv) = live_world(32_768);
    let a = started(&w, "a");
    report_after(&w, hv, Some((15, 100)));
    report_after(&w, hv, None);
    assert!(low_flag(&w));
    assert_eq!(count(&w, a, "node_disk_recovered"), 0);
}

#[test]
fn an_asleep_box_hears_its_node_going_low() {
    let (w, hv) = live_world(32_768);
    let a = started(&w, "a");
    idle(&w, a);
    let w = agents_report_idle(w, &[a]);
    report(&w, hv, SHORT, None);
    waits(&w, "b");
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Asleep);
    report_after(&w, hv, Some((15, 100)));
    assert_eq!(count(&w, a, "node_disk_low"), 1);
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Asleep);
}

#[test]
fn an_owner_with_two_boxes_on_the_node_hears_once() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    report_after(&w, hv, Some((15, 100)));
    assert_eq!(
        count(&w, a, "node_disk_low") + count(&w, b, "node_disk_low"),
        1
    );
}

#[test]
fn an_awake_box_carries_the_alert_for_an_owner_who_also_has_an_asleep_one() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    idle(&w, a);
    idle(&w, b);
    let w = agents_report_idle(w, &[a, b]);
    report(&w, hv, SHORT, None);
    waits(&w, "c");
    assert_eq!(lifecycle(&w, a), BoxLifecycle::Asleep);
    assert_eq!(lifecycle(&w, b), BoxLifecycle::Ready);
    report_after(&w, hv, Some((15, 100)));
    assert_eq!(count(&w, a, "node_disk_low"), 0);
    assert_eq!(count(&w, b, "node_disk_low"), 1);
}
