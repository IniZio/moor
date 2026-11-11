//! Promotions on separate nodes run apart.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use moor_core::config::{NodeConfig, UserConfig};
use moor_core::domain::hands::HandsState;
use moor_core::BoxId;

use super::{on_provider_node, row_of, start_pass, waiting_directly, world, World, PROVIDER};
use crate::testing::driver::FakeDriver;

/// Whether Box `id` holds a ready machine.
fn has_ready_machine(w: &World, id: BoxId) -> bool {
    row_of(w, id).hands.map(|b| b.state) == Some(HandsState::Ready)
}

/// Nodes `a` and `b` on the one provider, each with room for one Box.
fn two_nodes() -> UserConfig {
    let node = || NodeConfig {
        runtime: PROVIDER.to_owned(),
        labels: Vec::new(),
        max_boxes: NonZeroU32::new(1),
    };
    UserConfig {
        nodes: BTreeMap::from([("a".to_owned(), node()), ("b".to_owned(), node())]),
        ..UserConfig::default()
    }
}

#[test]
fn a_long_setup_on_one_node_holds_up_no_promotion_on_another() {
    let w = world(two_nodes(), FakeDriver::new());
    let on_a = waiting_directly(&w, "on-a");
    on_provider_node(&w.cx, on_a, "a");
    let on_b = waiting_directly(&w, "on-b");
    on_provider_node(&w.cx, on_b, "b");
    let (entered_tx, entered) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let first = Mutex::new(Some((entered_tx, released)));
    w.driver.notes_while_acquiring(&[], move || {
        let blocked = first.lock().unwrap().take();
        if let Some((entered, released)) = blocked {
            entered.send(()).unwrap();
            released.recv().ok();
        }
    });
    let started = start_pass(&w);
    assert_eq!(started.len(), 2);
    entered.recv_timeout(Duration::from_secs(30)).unwrap();
    let mut tries = 0;
    while !(has_ready_machine(&w, on_a) || has_ready_machine(&w, on_b)) {
        tries += 1;
        assert!(
            tries < 3000,
            "no node promoted while the other's setup blocked"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        start_pass(&w).is_empty(),
        "a node already promoting is promoted again"
    );
    release.send(()).unwrap();
    for promoting in started {
        promoting.join().unwrap();
    }
    assert!(has_ready_machine(&w, on_a) && has_ready_machine(&w, on_b));
}
