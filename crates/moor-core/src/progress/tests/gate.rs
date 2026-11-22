//! A Box with gates is never done while one is open.

use super::*;
use crate::gate::{CheckDoc, GateNext, GateStatus};
use crate::view::gate::GateView;
use crate::workflow::StepId;

fn gated(lifecycle: BoxLifecycle, session: SessionState, status: GateStatus) -> BoxView {
    let mut b = row(lifecycle, session);
    b.gate = Some(GateView {
        gates: vec!["proof".to_owned()],
        status,
        wait: None,
        note: None,
        run_ended: None,
    });
    b
}

#[test]
fn a_gated_box_first_message_shows_the_persons_words_without_the_preamble() {
    let gates = crate::gate::Gates::parse(
        &[crate::gate::GateDoc {
            name: "proof".to_owned(),
            evidence: vec!["proof.md".to_owned()],
            checks: Vec::new(),
            approval: None,
            max_tries: None,
        }],
        &[],
    )
    .unwrap();
    let start = gates.first_message("Fix the typo.");
    let events = [
        ev(1, "steered", "t", json!({"message": start})),
        ev(2, "steered", "t", json!({"message": "keep going"})),
    ];
    let [first, second] = timeline(&events).try_into().unwrap();
    assert_eq!(first.text, "Fix the typo.");
    assert!(!first.text.contains("report_step"));
    assert_eq!(second.text, "keep going");
}

fn proof() -> StepId {
    StepId::parse("proof").unwrap()
}

fn open(next: GateNext) -> GateStatus {
    GateStatus::Open {
        gate: proof(),
        try_n: 1,
        next,
    }
}

fn stuck() -> GateStatus {
    GateStatus::Stuck {
        gate: proof(),
        try_n: 1,
        why: "the agent went quiet".to_owned(),
        unanswered: None,
    }
}

#[test]
fn open_gate_is_never_done() {
    for session in [SessionState::Idle, SessionState::Ended] {
        let none = derive(&row(BoxLifecycle::Ready, session), &[], ten());
        assert_eq!(none.state, Progress::Done, "no gates: {session:?}");
        for next in [GateNext::Agent, GateNext::RunChecks] {
            let b = gated(BoxLifecycle::Ready, session, open(next));
            assert_eq!(derive(&b, &[], ten()).state, Progress::Working, "{next:?}");
        }
        let passed = gated(
            BoxLifecycle::Ready,
            session,
            GateStatus::Passed {
                outcome: serde_json::from_value(
                    serde_json::json!({"name": "done", "tone": "good"}),
                )
                .unwrap(),
            },
        );
        assert_eq!(derive(&passed, &[], ten()).state, Progress::Done);
    }
}

#[test]
fn an_idle_box_says_which_gate_it_waits_on() {
    let b = gated(
        BoxLifecycle::Ready,
        SessionState::Idle,
        open(GateNext::Agent),
    );
    let p = derive(&b, &[], ten());
    assert_eq!(p.line, "gate proof: waiting for the agent's evidence");
    let b = gated(
        BoxLifecycle::Ready,
        SessionState::Idle,
        open(GateNext::RunChecks),
    );
    assert_eq!(
        derive(&b, &[], ten()).line,
        "gate proof: running its checks"
    );
}

#[test]
fn a_working_agent_keeps_its_own_line_while_a_gate_is_open() {
    let b = gated(
        BoxLifecycle::Ready,
        SessionState::Working,
        open(GateNext::Agent),
    );
    let p = derive(&b, &[], ten());
    assert_eq!((p.state, p.line.as_str()), (Progress::Working, "working"));
}

#[test]
fn a_gate_that_waits_for_a_person_needs_you_even_when_the_box_sleeps() {
    for lifecycle in [BoxLifecycle::Ready, BoxLifecycle::Asleep] {
        let ask = gated(lifecycle, SessionState::Idle, open(GateNext::Ask));
        let p = derive(&ask, &[], ten());
        assert_eq!(
            (p.state, p.line.as_str()),
            (Progress::NeedsYou, "gate proof: waiting for your approval")
        );
        let stuck = gated(lifecycle, SessionState::Idle, stuck());
        let p = derive(&stuck, &[], ten());
        assert_eq!(
            (p.state, p.line.as_str()),
            (
                Progress::NeedsYou,
                "gate proof is stuck: the agent went quiet"
            )
        );
    }
}

#[test]
fn a_sleeping_box_with_a_gate_that_waits_on_the_agent_stays_asleep() {
    let b = gated(
        BoxLifecycle::Asleep,
        SessionState::Idle,
        open(GateNext::Agent),
    );
    assert_eq!(derive(&b, &[], ten()).state, Progress::Asleep);
}

#[test]
fn a_failed_gate_fails_the_box_and_says_why() {
    let failed = GateStatus::Failed {
        gate: proof(),
        why: "the owner said no".to_owned(),
    };
    let b = gated(BoxLifecycle::Ready, SessionState::Idle, failed);
    let p = derive(&b, &[], ten());
    assert_eq!(
        (p.state, p.failed, p.line.as_str()),
        (
            Progress::Failed,
            true,
            "gate proof failed: the owner said no"
        )
    );
}

#[test]
fn a_gate_never_revives_a_box_that_is_closed_or_failed() {
    for (lifecycle, want) in [
        (BoxLifecycle::Closed, Progress::Closed),
        (BoxLifecycle::SetupFailed, Progress::Failed),
    ] {
        let b = gated(lifecycle, SessionState::Idle, stuck());
        assert_eq!(derive(&b, &[], ten()).state, want);
    }
}

fn doc(evidence: &[&str], checks: &[&str], approval: bool) -> crate::gate::GateDoc {
    crate::gate::GateDoc {
        name: "proof".to_owned(),
        evidence: evidence.iter().map(|s| (*s).to_owned()).collect(),
        checks: checks.iter().map(|s| CheckDoc::from(*s)).collect(),
        approval: approval.then_some(crate::gate::Approval::Owner),
        max_tries: Some(1),
    }
}

fn standing(
    lifecycle: BoxLifecycle,
    d: &crate::gate::GateDoc,
    lines: &[crate::gate::GateLine],
) -> BoxView {
    let gates = crate::gate::Gates::parse(std::slice::from_ref(d), &[]).unwrap();
    let mut b = row(lifecycle, SessionState::Idle);
    b.gate = Some(GateView::of(&gates, lines));
    b
}

fn submitted() -> crate::gate::GateLine {
    crate::gate::GateLine::Submitted {
        gate: proof(),
        try_n: 1,
        report: crate::workflow::StepReport {
            step: proof(),
            result: crate::workflow::StepResult::Ok,
            evidence: vec!["proof.md".to_owned()],
            note: "all green".to_owned(),
        },
    }
}

#[test]
fn a_gate_that_asks_the_owner_shows_an_approve_card_with_what_was_handed_in() {
    let d = doc(&["proof.md"], &[], true);
    for lifecycle in [BoxLifecycle::Ready, BoxLifecycle::Asleep] {
        let b = standing(lifecycle, &d, &[submitted()]);
        let p = derive(&b, &[], ten());
        assert_eq!(p.state, Progress::NeedsYou, "{lifecycle:?}");
        let card = p.needs.unwrap();
        assert_eq!(card.kind, NeedKind::Approve);
        assert!(card.question.contains("proof"), "{}", card.question);
        assert!(card.question.contains("proof.md"), "{}", card.question);
        assert_eq!(card.detail.as_deref(), Some("all green"));
        assert!(card.picks.is_empty() && card.approve.is_none());
        let wait = card.gate.unwrap();
        assert_eq!((wait.gate.as_str(), wait.try_n), ("proof", Some(1)));
    }
}

#[test]
fn a_card_for_a_later_wait_has_another_id() {
    let d = crate::gate::GateDoc {
        max_tries: Some(2),
        ..doc(&[], &[], true)
    };
    let first = derive(&standing(BoxLifecycle::Ready, &d, &[]), &[], ten());
    let no = crate::gate::GateLine::Answered {
        gate: proof(),
        try_n: 1,
        by: "me".to_owned(),
        answer: crate::workflow::Answer::No,
        note: String::new(),
    };
    let second = derive(&standing(BoxLifecycle::Ready, &d, &[no]), &[], ten());
    let (a, b) = (first.needs.unwrap(), second.needs.unwrap());
    assert_eq!(
        (a.gate.unwrap().try_n, b.gate.unwrap().try_n),
        (Some(1), Some(2))
    );
    assert_ne!(a.id, b.id);
}

#[test]
fn a_stuck_gate_shows_a_card_with_why_and_the_failing_command() {
    let d = doc(&[], &["cargo test --workspace"], false);
    let failed = crate::gate::GateLine::Checked {
        gate: proof(),
        try_n: 1,
        check: 0,
        exit: 101,
        tail: "1 test failed".to_owned(),
    };
    for lifecycle in [BoxLifecycle::Ready, BoxLifecycle::Asleep] {
        let b = standing(lifecycle, &d, std::slice::from_ref(&failed));
        let p = derive(&b, &[], ten());
        assert_eq!(p.state, Progress::NeedsYou, "{lifecycle:?}");
        assert_eq!(
            p.line,
            "gate proof is stuck: out of tries: check 1 (`cargo test --workspace`) exited 101"
        );
        let card = p.needs.unwrap();
        assert_eq!(card.kind, NeedKind::Stuck);
        assert_eq!(card.detail.as_deref(), Some("cargo test --workspace"));
        let wait = card.gate.unwrap();
        assert_eq!(wait.gate, "proof");
        assert!(wait.why.unwrap().contains("exited 101"));
        assert_eq!(wait.command.as_deref(), Some("cargo test --workspace"));
    }
}

#[test]
fn a_stuck_check_with_a_sentence_puts_the_sentence_first_on_the_card_and_the_line() {
    let said = crate::gate::CheckDoc::Said(crate::gate::SaidDoc {
        run: "test -f never.txt".to_owned(),
        says: "The file never.txt exists.".to_owned(),
    });
    let d = crate::gate::GateDoc {
        checks: vec![said],
        ..doc(&[], &[], false)
    };
    let failed = crate::gate::GateLine::Checked {
        gate: proof(),
        try_n: 1,
        check: 0,
        exit: 1,
        tail: String::new(),
    };
    let p = derive(&standing(BoxLifecycle::Ready, &d, &[failed]), &[], ten());
    assert_eq!(
        p.line,
        "gate proof is stuck: out of tries: check 1 (“The file never.txt exists.”) exited 1"
    );
    let card = p.needs.unwrap();
    assert_eq!(card.detail.as_deref(), Some("The file never.txt exists."));
    let wait = card.gate.unwrap();
    assert_eq!(wait.command.as_deref(), Some("test -f never.txt"));
}

#[test]
fn a_stuck_card_carries_the_end_of_the_output_and_what_the_person_can_do() {
    let d = doc(&[], &["make"], false);
    let failed = crate::gate::GateLine::Checked {
        gate: proof(),
        try_n: 1,
        check: 0,
        exit: 2,
        tail: "1 test failed".to_owned(),
    };
    let b = standing(BoxLifecycle::Ready, &d, &[failed]);
    let wait = derive(&b, &[], ten()).needs.unwrap().gate.unwrap();
    assert_eq!(wait.tail.as_deref(), Some("1 test failed"));
    assert!(wait.hint.contains("reopen"));
}

#[test]
fn the_cards_json_names_its_kind_and_gate() {
    let d = doc(&[], &["make"], false);
    let failed = crate::gate::GateLine::NoAnswer {
        gate: proof(),
        try_n: 1,
        check: crate::gate::CheckOf::Gate { check: 0 },
        why: crate::gate::Unanswered::Lost,
    };
    let lines = [failed.clone(), failed];
    let p = derive(&standing(BoxLifecycle::Ready, &d, &lines), &[], ten());
    let json = serde_json::to_value(p.needs.unwrap()).unwrap();
    assert_eq!(json.get("kind"), Some(&json!("stuck")));
    assert_eq!(json.pointer("/gate/gate"), Some(&json!("proof")));
    assert_eq!(json.pointer("/gate/command"), Some(&json!("make")));
}

#[test]
fn the_needs_cards_json_is_pinned_for_both_kinds() {
    let ask = derive(
        &standing(
            BoxLifecycle::Ready,
            &doc(&["proof.md"], &[], true),
            &[submitted()],
        ),
        &[],
        ten(),
    );
    assert_eq!(
        serde_json::to_value(ask.needs.unwrap()).unwrap(),
        json!({
            "id": ask_id_of(&[submitted()]),
            "kind": "approve",
            "question": "Gate `proof` asks your approval. The agent handed in proof.md.",
            "detail": "all green",
            "picks": [],
            "other": null,
            "approve": null,
            "plan": false,
            "gate": {
                "gate": "proof", "kind": "approve", "seq": 1, "try_n": 1, "why": null,
                "handed": ["proof.md"], "note": "all green", "command": null, "tail": null,
                "missing": [],
                "hint": "Answer yes to pass the gate, or no to fail this try; a note goes to the agent."
            }
        })
    );
    let failed = crate::gate::GateLine::Checked {
        gate: proof(),
        try_n: 1,
        check: 0,
        exit: 2,
        tail: "boom".to_owned(),
    };
    let stuck = derive(
        &standing(BoxLifecycle::Ready, &doc(&[], &["make"], false), &[failed]),
        &[],
        ten(),
    );
    let card = serde_json::to_value(stuck.needs.unwrap()).unwrap();
    assert_eq!(card.get("kind"), Some(&json!("stuck")));
    assert_eq!(
        card.get("question"),
        Some(&json!(
            "Gate `proof` is stuck: out of tries: check 1 (`make`) exited 2"
        ))
    );
    assert_eq!(card.get("detail"), Some(&json!("make")));
    assert_eq!(card.get("picks"), Some(&json!([])));
    assert_eq!(card.pointer("/gate/kind"), Some(&json!("stuck")));
    assert_eq!(card.pointer("/gate/tail"), Some(&json!("boom")));
}

fn ask_id_of(lines: &[crate::gate::GateLine]) -> String {
    let d = doc(&["proof.md"], &[], true);
    let gates = crate::gate::Gates::parse(std::slice::from_ref(&d), &[]).unwrap();
    crate::progress::gate_ask_id(&crate::gate::wait(&gates, lines).unwrap())
}

#[test]
fn an_approval_only_gate_shows_working_until_the_agent_is_quiet() {
    let d = doc(&[], &[], true);
    for session in [SessionState::Starting, SessionState::Working] {
        let mut b = standing(BoxLifecycle::Ready, &d, &[]);
        b.session_state = session;
        let p = derive(&b, &[], ten());
        assert_eq!(p.state, Progress::Working, "{session:?}");
        assert!(p.needs.is_none(), "{session:?}");
    }
    for session in [SessionState::Idle, SessionState::Ended] {
        let mut b = standing(BoxLifecycle::Ready, &d, &[]);
        b.session_state = session;
        let p = derive(&b, &[], ten());
        assert_eq!(p.state, Progress::NeedsYou, "{session:?}");
        assert!(p.needs.is_some(), "{session:?}");
    }
}

fn said(text: &str) -> EventView {
    ev(1, "agent_said", "t", json!({ "text": text }))
}

#[test]
fn a_done_box_whose_gates_passed_says_the_note_of_the_last_accepted_report() {
    let d = doc(&["proof.md"], &[], false);
    let b = standing(BoxLifecycle::Ready, &d, &[submitted()]);
    let events = [said("I'm now waiting for the owner's answer.")];
    let p = derive(&b, &events, ten());
    assert_eq!((p.state, p.line.as_str()), (Progress::Done, "all green"));
}

#[test]
fn a_done_gated_box_without_a_report_note_keeps_the_agents_line() {
    let d = doc(&[], &["make"], false);
    let ran = crate::gate::GateLine::Checked {
        gate: proof(),
        try_n: 1,
        check: 0,
        exit: 0,
        tail: String::new(),
    };
    let b = standing(BoxLifecycle::Ready, &d, &[ran]);
    let events = [said("Finished the change.")];
    let p = derive(&b, &events, ten());
    assert_eq!(
        (p.state, p.line.as_str()),
        (Progress::Done, "Finished the change.")
    );
}
