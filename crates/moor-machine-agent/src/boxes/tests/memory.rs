//! The host memory each Box holds, as the agent reports it to the runner.

use moor_client::link::BoxMemory;

use super::*;

#[tokio::test]
async fn a_running_box_reports_what_it_uses_not_its_ceiling() {
    let rig = rig();
    rig.opened().await;
    rig.boxes.provider.set_mem(&slug(), 3 * 1024);
    assert_eq!(
        rig.boxes.memory().await,
        [BoxMemory {
            slug: s(SLUG),
            host_memory_mib: 3 * 1024,
            resume_memory_mib: None,
        }]
    );
}

#[tokio::test]
async fn an_asleep_box_holds_nothing_and_its_wake_asks_what_it_used_when_it_slept() {
    let rig = rig();
    rig.opened().await;
    rig.boxes.provider.set_mem(&slug(), 3 * 1024);
    assert_eq!(
        rig.one(&BoxOp::Sleep { slug: s(SLUG) }).await,
        BoxAnswer::Done
    );
    assert_eq!(
        rig.boxes.memory().await,
        [BoxMemory {
            slug: s(SLUG),
            host_memory_mib: 0,
            resume_memory_mib: Some(3 * 1024),
        }]
    );
}

#[tokio::test]
async fn a_running_box_its_provider_cannot_measure_is_left_out() {
    let rig = rig();
    rig.opened().await;
    assert_eq!(rig.boxes.memory().await, []);
}
