//! The room made for the head of a queue: a report taken before a Box stopped for it does not
//! show that room, so nothing behind the head may take it.

use super::*;

/// A head needing 16 GiB with 12 GiB free, asleep, Boxes asleep behind it with the memory
/// `behind` gives, and one Box up whose agent is busy.
fn head_short_of_room(behind: &[u64]) -> (World, i64, BoxId, BoxId, Vec<BoxId>) {
    let (w, hv) = live_world(32_768);
    let head = asleep(&w, "head", 16 * 1024);
    let others = behind
        .iter()
        .enumerate()
        .map(|(i, mib)| asleep(&w, &format!("behind-{i}"), *mib))
        .collect();
    let up = started(&w, "up");
    let (w, _) = agents_report(w, &[], &[up]);
    tick(&w, hv, with_free(12 * 1024), 0, 0);
    assert!(wake_up(&w, head).is_err());
    assert_eq!(
        reason(&w, head),
        "waiting for capacity on hv: 12 GiB free, needs 16 GiB"
    );
    (w, hv, head, up, others)
}

/// Lets `up`'s agent go idle, so the next report suspends it for the head.
fn up_goes_idle(w: World, hv: i64, up: BoxId) -> World {
    idle(&w, up);
    let (w, _) = agents_report(w, &[up], &[]);
    tick(&w, hv, with_free(12 * 1024), 0, 0);
    assert_eq!(lifecycle(&w, up), Asleep, "room made for the head");
    w
}

#[test]
fn a_later_start_waits_behind_a_head_needing_16_gib_with_12_free() {
    let (w, hv, head, up, _) = head_short_of_room(&[]);
    let (later, why) = waits(&w, "later");
    assert_eq!(why, "hv is making room; 1 Boxes ahead");
    let w = up_goes_idle(w, hv, up);
    assert_eq!(states(&w, &[head, later]), [Pending, Pending]);
    tick(&w, hv, with_free(18 * 1024), 0, 0);
    assert_eq!(states(&w, &[head, later]), [Ready, Pending]);
    tick(&w, hv, with_free(4 * 1024), 0, 0);
    assert_eq!(states(&w, &[head, later]), [Ready, Ready]);
}

#[test]
fn behind_the_head_a_wake_leaves_before_a_start_that_queued_first() {
    let (w, hv, head, up, behind) = head_short_of_room(&[1024]);
    let wake = behind.first().copied().unwrap();
    let (start, _) = waits(&w, "start");
    assert!(
        wake_up(&w, wake).is_err(),
        "room, but it is behind the head"
    );
    assert_eq!(reason(&w, wake), "hv is making room; 1 Boxes ahead");
    let w = up_goes_idle(w, hv, up);
    assert_eq!(
        states(&w, &[head, wake, start]),
        [Pending, Pending, Pending]
    );
    tick(&w, hv, with_free(18 * 1024), 0, 0);
    assert_eq!(states(&w, &[head, wake, start]), [Ready, Pending, Pending]);
    tick(&w, hv, with_free(4 * 1024), 0, 0);
    assert_eq!(states(&w, &[head, wake, start]), [Ready, Ready, Pending]);
    tick(&w, hv, with_free(4 * 1024), 0, 0);
    assert_eq!(states(&w, &[head, wake, start]), [Ready, Ready, Ready]);
}

#[test]
fn a_repeated_start_of_a_queued_box_keeps_it_behind_the_head() {
    let (w, _, head, _, _) = head_short_of_room(&[]);
    let (later, _) = waits(&w, "later");
    let again = start::run(&w.cx, &req("later")).unwrap();
    assert!(again.queued);
    assert_eq!(states(&w, &[head, later]), [Pending, Pending]);
}
