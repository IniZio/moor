//! Tests for picking the Box to suspend on a node short of memory.

use super::{longest_idle, Resident};
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::name::BoxId;
use crate::seam::clock::Ts;

fn idle(id: i64, since_ms: i64) -> Resident {
    Resident {
        id: BoxId::new(id),
        lifecycle: BoxLifecycle::Ready,
        session: SessionState::Idle,
        needs_input: false,
        idle_since: Some(Ts::from_millis(since_ms)),
    }
}

#[test]
fn picks_exactly_the_longest_idle() {
    let boxes = [idle(1, 300), idle(2, 100), idle(3, 200)];
    assert_eq!(longest_idle(&boxes), Some(BoxId::new(2)));
}

#[test]
fn never_picks_a_working_waiting_or_starting_box() {
    let working = Resident {
        session: SessionState::Working,
        ..idle(1, 0)
    };
    let asking = Resident {
        needs_input: true,
        ..idle(2, 0)
    };
    let starting = Resident {
        lifecycle: BoxLifecycle::SettingUp,
        session: SessionState::Starting,
        ..idle(3, 0)
    };
    let fresh_session = Resident {
        session: SessionState::Starting,
        ..idle(4, 0)
    };
    let busy = [working, asking, starting, fresh_session];
    assert_eq!(longest_idle(&busy), None);
    let mut all = busy.to_vec();
    all.push(idle(5, 50));
    assert_eq!(longest_idle(&all), Some(BoxId::new(5)));
}
