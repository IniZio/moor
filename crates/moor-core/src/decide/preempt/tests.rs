//! Tests for making room on a full node.

use super::{make_room, Resident, Room};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::name::BoxId;
use crate::seam::clock::Ts;

fn idle(id: i64, since_ms: i64, mem_mib: u64) -> Resident {
    Resident {
        id: BoxId::new(id),
        lifecycle: BoxLifecycle::Ready,
        session: SessionState::Idle,
        needs_input: false,
        idle_since: Some(Ts::from_millis(since_ms)),
        mem_mib,
    }
}

#[test]
fn fits_without_suspending() {
    assert_eq!(make_room(4096, 4096, &[idle(1, 0, 4096)]), Room::Fits);
}

#[test]
fn picks_the_longest_idle_first() {
    let boxes = [idle(1, 300, 4096), idle(2, 100, 4096), idle(3, 200, 4096)];
    assert_eq!(
        make_room(0, 4096, &boxes),
        Room::Suspend(vec![BoxId::new(2)])
    );
    assert_eq!(
        make_room(0, 8192, &boxes),
        Room::Suspend(vec![BoxId::new(2), BoxId::new(3)])
    );
}

#[test]
fn never_picks_a_working_waiting_or_starting_box() {
    let working = Resident {
        session: SessionState::Working,
        ..idle(1, 0, 8192)
    };
    let asking = Resident {
        needs_input: true,
        ..idle(2, 0, 8192)
    };
    let starting = Resident {
        lifecycle: BoxLifecycle::SettingUp,
        session: SessionState::Starting,
        ..idle(3, 0, 8192)
    };
    let fresh_session = Resident {
        session: SessionState::Starting,
        ..idle(4, 0, 8192)
    };
    let boxes = [working, asking, starting, fresh_session, idle(5, 50, 4096)];
    assert_eq!(
        make_room(0, 4096, &boxes),
        Room::Suspend(vec![BoxId::new(5)])
    );
    assert_eq!(make_room(0, 8192, &boxes), Room::Short);
}

#[test]
fn suspends_nothing_when_all_idle_boxes_would_not_make_room() {
    let boxes = [idle(1, 0, 2048), idle(2, 10, 2048)];
    assert_eq!(make_room(1024, 8192, &boxes), Room::Short);
}
