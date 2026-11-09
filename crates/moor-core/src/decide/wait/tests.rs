//! Tests for the wait verdict.

use std::time::Duration;

use super::{wait_verdict, WaitFacts, WaitVerdict};
use crate::request::WaitFor;

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

const fn facts(agent_active: bool, run_in_flight: bool) -> WaitFacts {
    WaitFacts {
        setting_up: false,
        agent_active,
        run_in_flight,
    }
}

const fn setting_up() -> WaitFacts {
    WaitFacts {
        setting_up: true,
        agent_active: false,
        run_in_flight: false,
    }
}

#[test]
fn verdict_table() {
    use WaitFor::{Done, Idle, Input, Run};
    use WaitVerdict as V;
    let cases = [
        // (facts, want, elapsed, timeout, expected)
        (facts(false, false), Idle, 0, 10, V::Done),
        (facts(true, false), Idle, 0, 10, V::Keep),
        (facts(false, true), Idle, 0, 10, V::Keep),
        (facts(false, false), Done, 0, 10, V::Done),
        (facts(true, false), Done, 0, 10, V::Keep),
        (facts(false, true), Done, 0, 10, V::Keep),
        (facts(false, false), Run, 0, 10, V::Done),
        (facts(true, false), Run, 0, 10, V::Done),
        (facts(true, true), Run, 0, 10, V::Keep),
        (facts(true, true), Run, 9, 10, V::Keep),
        (facts(true, true), Run, 10, 10, V::TimedOut),
        (facts(true, false), Idle, 9, 10, V::Keep),
        (facts(true, false), Idle, 10, 10, V::TimedOut),
        (facts(true, false), Idle, 11, 10, V::TimedOut),
        (facts(false, false), Done, 10, 10, V::Done),
        (facts(false, false), Run, 99, 10, V::Done),
        (facts(true, false), Idle, 0, 0, V::TimedOut),
        (facts(false, false), Input, 0, 10, V::Done),
        (facts(true, false), Input, 0, 10, V::Keep),
        (facts(true, false), Input, 10, 10, V::TimedOut),
        (setting_up(), Idle, 0, 10, V::Keep),
        (setting_up(), Done, 0, 10, V::Keep),
        (setting_up(), Input, 0, 10, V::Keep),
        (setting_up(), Idle, 10, 10, V::TimedOut),
        (setting_up(), Run, 0, 10, V::Done),
    ];
    for (f, want, elapsed, timeout, expected) in cases {
        assert_eq!(
            wait_verdict(f, want, secs(elapsed), secs(timeout)),
            expected,
            "{f:?} {want:?} {elapsed}/{timeout}"
        );
    }
}
