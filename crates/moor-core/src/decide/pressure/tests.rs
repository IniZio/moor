use super::{relieve, Candidate, Cooldown, Demand, Reading, Suspend, COOLDOWN_SECS};
use crate::decide::preempt::Resident;
use crate::domain::box_state::BoxLifecycle;
use crate::domain::session_state::SessionState;
use crate::name::{BoxId, NodeName};
use crate::seam::clock::Ts;

const RESERVE: u64 = 4096;
const PLENTY: u64 = 32_768;

fn node() -> NodeName {
    NodeName::parse("hv").unwrap()
}

fn reading(at: i64, available_mib: u64, some: u32, full: u32) -> Reading {
    Reading {
        at,
        available_mib,
        psi_some_centi: Some(some),
        psi_full_centi: Some(full),
    }
}

fn pressed(n: i64) -> Vec<Reading> {
    (0..n).map(|i| reading(i * 10, PLENTY, 1000, 0)).collect()
}

fn of(id: i64, session: SessionState, idle_ms: Option<i64>, started_at: i64) -> Candidate {
    Candidate {
        resident: Resident {
            id: BoxId::new(id),
            lifecycle: BoxLifecycle::Ready,
            session,
            needs_input: false,
            idle_since: idle_ms.map(Ts::from_millis),
        },
        started_at,
    }
}

fn idle(id: i64, since_ms: i64) -> Candidate {
    of(id, SessionState::Idle, Some(since_ms), 0)
}

fn working(id: i64, started_at: i64) -> Candidate {
    of(id, SessionState::Working, None, started_at)
}

fn waiting(id: i64) -> Candidate {
    let mut c = working(id, 1_000);
    c.resident.needs_input = true;
    c
}

fn starting(id: i64) -> Candidate {
    let mut c = of(id, SessionState::Starting, None, 2_000);
    c.resident.lifecycle = BoxLifecycle::SettingUp;
    c
}

fn pick(readings: &[Reading], boxes: &[Candidate], suspended_at: Option<i64>) -> Option<Suspend> {
    let now = readings.last().map_or(0, |r| r.at);
    relieve(
        &node(),
        readings,
        RESERVE,
        boxes,
        Cooldown { suspended_at, now },
        &Demand::default(),
    )
}

fn crowded(waiter: Option<&str>) -> Demand {
    Demand {
        short: true,
        waiter: waiter.map(str::to_owned),
    }
}

fn pick_for(
    demand: &Demand,
    readings: &[Reading],
    boxes: &[Candidate],
    suspended_at: Option<i64>,
) -> Option<Suspend> {
    let now = readings.last().map_or(0, |r| r.at);
    relieve(
        &node(),
        readings,
        RESERVE,
        boxes,
        Cooldown { suspended_at, now },
        demand,
    )
}

fn quiet() -> Vec<Reading> {
    vec![reading(100, PLENTY, 0, 0)]
}

#[test]
fn psi_counts_as_sustained_only_on_the_third_report_in_a_row() {
    let boxes = [idle(1, 10)];
    assert_eq!(pick(&pressed(2), &boxes, None), None);
    let mut broken = pressed(3);
    if let Some(r) = broken.get_mut(1) {
        r.psi_some_centi = Some(999);
    }
    assert_eq!(pick(&broken, &boxes, None), None);
    assert_eq!(
        pick(&pressed(3), &boxes, None).map(|s| s.id),
        Some(BoxId::new(1))
    );
}

#[test]
fn less_available_than_the_reserve_is_sustained_at_once() {
    let low = [reading(0, RESERVE - 1, 0, 0)];
    assert!(pick(&low, &[idle(1, 10)], None).is_some());
}

#[test]
fn suspends_exactly_the_longest_idle_box_with_the_pressure_reason() {
    let boxes = [idle(1, 50), idle(2, 10), idle(3, 30)];
    assert_eq!(
        pick(&pressed(3), &boxes, None),
        Some(Suspend {
            id: BoxId::new(2),
            last_resort: false,
            reason: "memory pressure on hv".to_owned(),
        })
    );
}

#[test]
fn a_suspension_holds_the_next_one_off_for_the_cooldown() {
    let readings = pressed(4);
    let now = readings.last().map_or(0, |r| r.at);
    let boxes = [idle(1, 10)];
    assert_eq!(pick(&readings, &boxes, Some(now - COOLDOWN_SECS + 1)), None);
    assert!(pick(&readings, &boxes, Some(now - COOLDOWN_SECS)).is_some());
}

#[test]
fn sustained_pressure_never_picks_a_working_waiting_or_starting_box() {
    let boxes = [working(1, 10), waiting(2), starting(3)];
    assert_eq!(pick(&pressed(5), &boxes, None), None);
}

#[test]
fn critical_with_nothing_idle_suspends_the_most_recently_started_working_box() {
    let boxes = [working(1, 10), working(2, 30), waiting(3), starting(4)];
    let expected = Some(Suspend {
        id: BoxId::new(2),
        last_resort: true,
        reason: "last-resort suspend: memory critical on hv".to_owned(),
    });
    assert_eq!(pick(&[reading(0, 512, 0, 0)], &boxes, None), expected);
    assert_eq!(pick(&[reading(0, PLENTY, 0, 500)], &boxes, None), expected);
}

#[test]
fn critical_still_prefers_an_idle_box() {
    let boxes = [working(1, 10), idle(2, 5)];
    let got = pick(&[reading(0, 512, 0, 0)], &boxes, None).unwrap();
    assert_eq!((got.id, got.last_resort), (BoxId::new(2), false));
}

#[test]
fn critical_never_picks_a_waiting_or_starting_box() {
    let boxes = [waiting(1), starting(2)];
    assert_eq!(pick(&[reading(0, 512, 0, 900)], &boxes, None), None);
}

#[test]
fn critical_respects_the_cooldown() {
    let boxes = [working(1, 10)];
    assert_eq!(pick(&[reading(100, 512, 0, 0)], &boxes, Some(90)), None);
}

#[test]
fn a_short_node_with_a_waiter_suspends_the_longest_idle_box_without_pressure() {
    let boxes = [idle(1, 50), idle(2, 10)];
    assert_eq!(
        pick_for(&crowded(Some("c")), &quiet(), &boxes, None),
        Some(Suspend {
            id: BoxId::new(2),
            last_resort: false,
            reason: "made room for c".to_owned(),
        })
    );
}

#[test]
fn a_short_node_with_no_waiter_and_no_pressure_suspends_nothing() {
    let boxes = [idle(1, 50), idle(2, 10)];
    assert_eq!(pick_for(&crowded(None), &quiet(), &boxes, None), None);
}

#[test]
fn a_node_with_room_and_a_waiter_suspends_nothing() {
    let boxes = [idle(1, 50), idle(2, 10)];
    let demand = Demand {
        short: false,
        waiter: Some("c".to_owned()),
    };
    assert_eq!(pick_for(&demand, &quiet(), &boxes, None), None);
}

#[test]
fn a_waiter_never_gets_a_working_box_suspended() {
    let boxes = [working(1, 10), waiting(2), starting(3)];
    assert_eq!(pick_for(&crowded(Some("c")), &quiet(), &boxes, None), None);
}

#[test]
fn the_cooldown_holds_a_waiter_suspension_off_too() {
    let boxes = [idle(1, 10)];
    let demand = crowded(Some("c"));
    assert_eq!(
        pick_for(&demand, &quiet(), &boxes, Some(100 - COOLDOWN_SECS + 1)),
        None
    );
    assert!(pick_for(&demand, &quiet(), &boxes, Some(100 - COOLDOWN_SECS)).is_some());
}

#[test]
fn pressure_wins_the_reason_when_both_hold() {
    let got = pick_for(&crowded(Some("c")), &pressed(3), &[idle(1, 10)], None).unwrap();
    assert_eq!(got.reason, "memory pressure on hv");
}
