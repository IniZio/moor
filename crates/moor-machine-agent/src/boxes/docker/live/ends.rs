//! A command whose exec stream ends is ended inside the Box, in either place.

#![allow(
    clippy::panic,
    clippy::missing_docs_in_private_items,
    reason = "tests assert on fixed fixtures"
)]

use super::*;

/// Its grandchild too; a command another stream started in the same place keeps running.
pub(super) async fn a_dropped_stream_ends_its_command_inside_the_box(rig: &Rig, names: &Names) {
    step("dropped stream");
    for (place, container) in [(Place::Work, &names.work), (Place::Agent, &names.agent)] {
        let kept = streaming(rig, &exec(MAIN, place, "sh -c 'sleep 701; :'", None, false)).await;
        let dropped = streaming(rig, &exec(MAIN, place, "sh -c 'sleep 601; :'", None, false)).await;
        within(Duration::from_secs(20), || runs(container, "sleep 601"))
            .await
            .unwrap_or_else(|()| panic!("{place:?}: the command never started"));
        drop(dropped);
        within(Duration::from_secs(10), || !runs(container, "sleep 601"))
            .await
            .unwrap_or_else(|()| panic!("{place:?}: the command outlived its stream"));
        assert!(
            runs(container, "sleep 701"),
            "{place:?}: the other command ended"
        );
        drop(kept);
        within(Duration::from_secs(10), || !runs(container, "sleep 701"))
            .await
            .unwrap_or_else(|()| panic!("{place:?}: the other command outlived its stream"));
    }
}

/// Whether a process whose whole command line is `line` runs in `container`.
fn runs(container: &str, line: &str) -> bool {
    docker(&["exec", container, "pgrep", "-fx", line])
}

/// Waits up to `limit` for `holds` to be true.
async fn within(limit: Duration, holds: impl Fn() -> bool) -> Result<(), ()> {
    let deadline = tokio::time::Instant::now() + limit;
    while !holds() {
        if tokio::time::Instant::now() >= deadline {
            return Err(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Ok(())
}
