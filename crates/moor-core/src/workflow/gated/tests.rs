//! The move of a run of a Box with gates, for each place the gates can stand.

use super::{gated_move, GatedMove, Standing};
use crate::gate::{GateNext, GateStatus};
use crate::workflow::StepId;

fn id(s: &str) -> StepId {
    StepId::parse(s).unwrap()
}

/// A run at `work`, try 1, working or waiting, in a Box that is up and quiet.
fn stands(at: &StepId, waiting: bool) -> Standing<'_> {
    Standing {
        at: (at, 1),
        waiting,
        box_open: true,
        quiet: true,
    }
}

fn open(gate: &str, try_n: u8, next: GateNext) -> GateStatus {
    GateStatus::Open {
        gate: id(gate),
        try_n,
        next,
    }
}

fn stuck(gate: &str) -> GateStatus {
    GateStatus::Stuck {
        gate: id(gate),
        why: "the agent went quiet".to_owned(),
    }
}

#[test]
fn the_agent_going_quiet_at_an_open_gate_moves_nothing() {
    let at = id("proof");
    for next in [GateNext::Agent, GateNext::RunChecks] {
        let status = open("proof", 1, next);
        assert_eq!(gated_move(stands(&at, false), &status), GatedMove::Stay);
    }
}

#[test]
fn a_run_follows_the_gate_in_hand() {
    let at = id("work");
    assert_eq!(
        gated_move(stands(&at, false), &open("proof", 1, GateNext::Agent)),
        GatedMove::Enter {
            gate: id("proof"),
            try_n: 1
        }
    );
    let at = id("proof");
    assert_eq!(
        gated_move(stands(&at, false), &open("proof", 2, GateNext::Agent)),
        GatedMove::Enter {
            gate: id("proof"),
            try_n: 2
        }
    );
}

#[test]
fn passed_delivers_and_failed_fails_whatever_the_box_or_the_wait() {
    let at = id("proof");
    for waiting in [false, true] {
        for box_open in [false, true] {
            let now = Standing {
                waiting,
                box_open,
                ..stands(&at, false)
            };
            assert_eq!(gated_move(now, &GateStatus::Passed), GatedMove::Deliver);
            let failed = GateStatus::Failed {
                gate: id("ship"),
                why: "the owner said no".to_owned(),
            };
            assert_eq!(
                gated_move(now, &failed),
                GatedMove::Fail {
                    gate: id("ship"),
                    reason: "the owner said no".to_owned()
                }
            );
        }
    }
}

#[test]
fn a_stuck_gate_holds_a_working_run_and_keeps_a_waiting_one() {
    let at = id("proof");
    assert_eq!(
        gated_move(stands(&at, false), &stuck("proof")),
        GatedMove::Wait {
            gate: id("proof"),
            try_n: 1
        }
    );
    assert_eq!(
        gated_move(stands(&at, true), &stuck("proof")),
        GatedMove::Stay
    );
}

#[test]
fn a_gate_that_asks_holds_the_run_once_the_agent_is_quiet() {
    let at = id("ship");
    let asks = open("ship", 1, GateNext::Ask);
    let busy = Standing {
        quiet: false,
        ..stands(&at, false)
    };
    assert_eq!(gated_move(busy, &asks), GatedMove::Stay);
    assert_eq!(
        gated_move(stands(&at, false), &asks),
        GatedMove::Wait {
            gate: id("ship"),
            try_n: 1
        }
    );
    assert_eq!(gated_move(stands(&at, true), &asks), GatedMove::Stay);
}

#[test]
fn a_wait_ends_when_the_gate_opens_for_the_agent_again() {
    let at = id("proof");
    for next in [GateNext::Agent, GateNext::RunChecks] {
        assert_eq!(
            gated_move(stands(&at, true), &open("proof", 1, next)),
            GatedMove::Resume {
                gate: id("proof"),
                try_n: 1
            }
        );
    }
    assert_eq!(
        gated_move(stands(&at, true), &open("ship", 1, GateNext::Agent)),
        GatedMove::Resume {
            gate: id("ship"),
            try_n: 1
        }
    );
}

#[test]
fn a_box_that_is_gone_ends_a_run_with_gates_not_passed() {
    let at = id("proof");
    for waiting in [false, true] {
        let now = Standing {
            box_open: false,
            waiting,
            ..stands(&at, false)
        };
        for status in [open("proof", 1, GateNext::Agent), stuck("proof")] {
            assert_eq!(gated_move(now, &status), GatedMove::Incomplete);
        }
    }
}

#[test]
fn a_run_stuck_at_its_gate_waits_at_the_try_it_was_at() {
    let at = id("proof");
    let standing = Standing {
        at: (&at, 2),
        ..stands(&at, false)
    };
    assert_eq!(
        gated_move(standing, &stuck("proof")),
        GatedMove::Wait {
            gate: id("proof"),
            try_n: 2
        }
    );
}
