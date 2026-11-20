//! A node whose agent measures what each Box uses: a running Box counts at its use plus 1 GiB of
//! headroom, and a wake asks what the Box used when it slept, not its ceiling.

use super::*;

/// The agent's per-Box memory: `(Box, MiB it holds, MiB its wake needs)`.
fn agent_measures(w: &World, hv: i64, boxes: &[(BoxId, u64, Option<u64>)]) {
    let store = w.cx.store().unwrap();
    let controller = store.controller_id().unwrap();
    let said: Vec<BoxHostMemory> = boxes
        .iter()
        .map(
            |&(box_id, host_memory_mib, resume_memory_mib)| BoxHostMemory {
                machine_id: hv,
                controller: controller.clone(),
                box_id,
                host_memory_mib,
                resume_memory_mib,
                inc: None,
            },
        )
        .collect();
    store
        .set_box_memory(&crate::testing::runner(), &said)
        .unwrap();
}

/// A Box with a 16 GiB ceiling, up and working.
fn working_16_gib(w: World, name: &str) -> (World, BoxId) {
    let id = started(&w, name);
    guest_memory(&w, id, 16 * 1024);
    let (w, _) = agents_report(w, &[], &[id]);
    (w, id)
}

#[test]
fn a_16_gib_box_using_3_gib_leaves_room_for_another_16_gib_box_to_wake() {
    let (w, hv) = live_world(32_768);
    let next = asleep(&w, "next", 16 * 1024);
    let (w, big) = working_16_gib(w, "big");
    agent_measures(&w, hv, &[(big, 3 * 1024, None)]);
    tick(&w, hv, with_free(8 * 1024), 0, 0);
    assert!(
        wake_up(&w, next).is_err(),
        "unmeasured, it asks its ceiling"
    );
    assert_eq!(
        reason(&w, next),
        "waiting for capacity on hv: 7 GiB free, needs 16 GiB"
    );
    agent_measures(&w, hv, &[(big, 3 * 1024, None), (next, 0, Some(3 * 1024))]);
    tick(&w, hv, with_free(8 * 1024), 0, 0);
    assert_eq!(states(&w, &[big, next]), [Ready, Ready]);
}

#[test]
fn a_running_box_keeps_1_gib_of_headroom_a_new_start_cannot_take() {
    let (w, hv) = live_world(32_768);
    let (w, big) = working_16_gib(w, "big");
    agent_measures(&w, hv, &[(big, 3 * 1024, None)]);
    tick(&w, hv, with_free(2560), 0, 0);
    let (late, why) = waits(&w, "late");
    assert_eq!(why, "waiting for capacity on hv: 1.5 GiB free, needs 2 GiB");
    tick(&w, hv, with_free(3 * 1024), 0, 0);
    assert_eq!(states(&w, &[big, late]), [Ready, Ready]);
}

#[test]
fn headroom_stops_at_the_ceiling() {
    let (w, hv) = live_world(32_768);
    let (w, big) = working_16_gib(w, "big");
    agent_measures(&w, hv, &[(big, 16 * 1024 - 512, None)]);
    tick(&w, hv, with_free(2560), 0, 0);
    let fits = start::run(&w.cx, &req("fits")).unwrap();
    assert!(!fits.queued, "512 MiB of headroom leaves 2 GiB free");
}
