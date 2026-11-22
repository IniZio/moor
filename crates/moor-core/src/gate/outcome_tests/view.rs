//! How outcomes reach the views: the implicit one, the passed-gate count and the closing wait.

use super::*;

#[test]
fn only_the_outcome_of_a_definition_with_none_is_implicit() {
    assert!(Outcome::implicit().verdict().is_implicit());
    assert!(!won("check passed").is_implicit(), "a named outcome");
    let named_done = Verdict {
        says: Some("Every gate passed.".to_owned()),
        ..Outcome::implicit().verdict()
    };
    assert!(!named_done.is_implicit(), "one that says something");
    let bad_done = Verdict {
        tone: Tone::Bad,
        ..Outcome::implicit().verdict()
    };
    assert!(!bad_done.is_implicit());
}

#[test]
fn a_view_counts_the_gates_that_passed_before_an_anytime_outcome_ended_the_run() {
    use crate::view::gate::GateView;
    let g = fixture();
    let early = GateView::of(&g, &[held("proof", 1, "nothing to do")]);
    assert_eq!((early.gates.len(), early.gates_passed), (2, 0));
    let late = [through_gates(), vec![held(CLOSING, 1, "check failed")]].concat();
    assert_eq!(GateView::of(&g, &late).gates_passed, 2);
    assert_eq!(GateView::of(&g, &[submitted("proof")]).gates_passed, 1);
}

#[test]
fn a_stuck_closing_stage_lists_each_outcome_tried_with_its_check_and_output() {
    let docs = [
        OutcomeDoc {
            says: Some("The check passed.".to_owned()),
            ..outcome("check passed", &["gh api passed"], false, None)
        },
        outcome("check failed", &["false", "true"], false, Some(Tone::Bad)),
    ];
    let g = Gates::parse(&gate_docs(), &docs).unwrap();
    let tried = |try_n| GateLine::NoneHeld {
        gate: closing(),
        try_n,
        tried: vec![
            Tried {
                outcome: OutcomeName("check passed".to_owned()),
                check: 0,
                exit: 1,
                tail: "state is failure".to_owned(),
            },
            Tried {
                outcome: OutcomeName("check failed".to_owned()),
                check: 0,
                exit: 1,
                tail: String::new(),
            },
        ],
    };
    let lines = [through_gates(), vec![tried(1), tried(2)]].concat();
    let w = wait(&g, &lines).unwrap();
    assert_eq!(w.tried.len(), 2);
    assert_eq!(
        w.tried[0],
        WaitTried {
            outcome: "check passed".to_owned(),
            says: Some("The check passed.".to_owned()),
            command: Some("gh api passed".to_owned()),
            exit: 1,
            tail: "state is failure".to_owned(),
        }
    );
    assert_eq!(
        (
            w.tried[1].says.as_deref(),
            w.tried[1].command.as_deref(),
            w.tried[1].exit
        ),
        (None, Some("false"), 1)
    );
    let json = serde_json::to_value(&w).unwrap();
    assert_eq!(json["tried"][0]["outcome"], "check passed");
    let gate_stuck = wait(&fixture(), &[submitted("proof")]);
    assert_eq!(gate_stuck, None, "a gate that is not stuck lists nothing");
}
