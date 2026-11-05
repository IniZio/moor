use super::{judge, Change, Report, State};

const TOTAL: u64 = 400 * 1024;
const FLOOR: u64 = 40 * 1024;
const MARGIN: u64 = 20 * 1024;
const NOW: i64 = 1_000;

fn report(free_mib: u64) -> Report {
    Report {
        at: NOW,
        free_mib: Some(free_mib),
        total_mib: Some(TOTAL),
    }
}

#[test]
fn crossing_below_the_floor_goes_low_once() {
    let (state, change) = judge(State::Ok, report(FLOOR - 1), NOW);
    assert_eq!(state, State::Low);
    assert_eq!(
        change,
        Some(Change::WentLow {
            free_mib: FLOOR - 1,
            total_mib: TOTAL
        })
    );
}

#[test]
fn at_the_floor_is_not_low() {
    assert_eq!(judge(State::Ok, report(FLOOR), NOW), (State::Ok, None));
}

#[test]
fn staying_low_says_nothing_more() {
    assert_eq!(
        judge(State::Low, report(FLOOR - 5), NOW),
        (State::Low, None)
    );
    assert_eq!(judge(State::Low, report(0), NOW), (State::Low, None));
}

#[test]
fn climbing_just_past_the_floor_is_not_recovery() {
    assert_eq!(
        judge(State::Low, report(FLOOR + 1), NOW),
        (State::Low, None)
    );
    assert_eq!(
        judge(State::Low, report(FLOOR + MARGIN - 1), NOW),
        (State::Low, None)
    );
}

#[test]
fn recovery_needs_the_margin() {
    let (state, change) = judge(State::Low, report(FLOOR + MARGIN), NOW);
    assert_eq!(state, State::Ok);
    assert_eq!(
        change,
        Some(Change::Recovered {
            free_mib: FLOOR + MARGIN,
            total_mib: TOTAL
        })
    );
}

#[test]
fn a_stale_report_changes_nothing() {
    let stale = Report {
        at: NOW - 61,
        ..report(0)
    };
    assert_eq!(judge(State::Ok, stale, NOW), (State::Ok, None));
    let stale = Report {
        at: NOW - 61,
        ..report(TOTAL)
    };
    assert_eq!(judge(State::Low, stale, NOW), (State::Low, None));
}

#[test]
fn a_node_without_disk_figures_changes_nothing() {
    let blank = Report {
        at: NOW,
        free_mib: None,
        total_mib: None,
    };
    assert_eq!(judge(State::Ok, blank, NOW), (State::Ok, None));
    assert_eq!(judge(State::Low, blank, NOW), (State::Low, None));
    let no_total = Report {
        total_mib: None,
        ..report(0)
    };
    assert_eq!(judge(State::Ok, no_total, NOW), (State::Ok, None));
}

#[test]
fn a_small_disk_uses_the_absolute_floor() {
    let small = Report {
        at: NOW,
        free_mib: Some(19 * 1024),
        total_mib: Some(100 * 1024),
    };
    assert_eq!(judge(State::Ok, small, NOW).0, State::Low);
}
