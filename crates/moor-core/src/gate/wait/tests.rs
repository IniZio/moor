//! What a waiting gate shows a person.

use super::*;
use crate::gate::{Approval, CheckDoc, GateDoc};
use crate::workflow::{Answer, StepReport};

fn doc(name: &str, evidence: &[&str], checks: &[&str], tries: u64) -> GateDoc {
    GateDoc {
        name: name.to_owned(),
        evidence: evidence.iter().map(|s| (*s).to_owned()).collect(),
        checks: checks.iter().map(|s| CheckDoc::from(*s)).collect(),
        approval: Some(Approval::Owner),
        max_tries: Some(tries),
    }
}

fn id(s: &str) -> StepId {
    StepId::parse(s).unwrap()
}

fn submitted(try_n: u8, note: &str) -> GateLine {
    GateLine::Submitted {
        gate: id("proof"),
        try_n,
        report: StepReport {
            step: id("proof"),
            result: StepResult::Ok,
            evidence: vec!["proof.md".to_owned(), "shot.png".to_owned()],
            note: note.to_owned(),
        },
    }
}

fn failed_check(try_n: u8) -> GateLine {
    GateLine::Checked {
        gate: id("proof"),
        try_n,
        check: 0,
        exit: 101,
        tail: "1 test failed\n".to_owned(),
    }
}

#[test]
fn an_open_question_carries_what_the_agent_handed_in() {
    let g = Gates::parse(&[doc("proof", &["proof.md", "shot.png"], &[], 2)], &[]).unwrap();
    assert_eq!(wait(&g, &[]), None);
    let lines = [submitted(1, "all green")];
    let w = wait(&g, &lines).unwrap();
    assert_eq!(
        (w.gate.as_str(), w.kind, w.try_n),
        ("proof", WaitKind::Approve, Some(1))
    );
    assert_eq!(w.handed, ["proof.md", "shot.png"]);
    assert_eq!(w.note.as_deref(), Some("all green"));
    assert_eq!(w.seq, 1);
}

#[test]
fn a_stuck_gate_carries_why_and_the_command_that_failed() {
    let g = Gates::parse(&[doc("proof", &[], &["cargo test --workspace"], 1)], &[]).unwrap();
    let lines = [failed_check(1)];
    let w = wait(&g, &lines).unwrap();
    assert_eq!(
        (w.gate.as_str(), w.kind, w.try_n),
        ("proof", WaitKind::Stuck, None)
    );
    assert_eq!(
        w.why.as_deref(),
        Some("out of tries: check 1 (`cargo test --workspace`) exited 101")
    );
    assert_eq!(w.command.as_deref(), Some("cargo test --workspace"));
    assert_eq!(w.tail.as_deref(), Some("1 test failed\n"));
    assert!(w.hint.contains("reopen"), "{}", w.hint);
}

#[test]
fn a_stuck_gate_names_the_evidence_it_never_got() {
    let g = Gates::parse(&[doc("proof", &["proof.md"], &[], 1)], &[]).unwrap();
    let refused = || GateLine::Refused {
        gate: id("proof"),
        try_n: 1,
        missing: vec!["proof.md".to_owned()],
    };
    let w = wait(&g, &[refused(), refused()]).unwrap();
    assert_eq!(w.kind, WaitKind::Stuck);
    assert_eq!(w.missing, ["proof.md"]);
    assert_eq!(w.command, None);
}

#[test]
fn a_quiet_agent_leaves_a_stuck_wait_with_only_a_reason() {
    let g = Gates::parse(&[doc("proof", &["proof.md"], &[], 1)], &[]).unwrap();
    let lines = [
        GateLine::Reminded {
            gate: id("proof"),
            try_n: 1,
        },
        GateLine::Quiet {
            gate: id("proof"),
            try_n: 1,
        },
    ];
    let w = wait(&g, &lines).unwrap();
    assert_eq!(w.why.as_deref(), Some("the agent went quiet"));
    assert_eq!((w.command, w.tail, w.missing.len()), (None, None, 0));
}

#[test]
fn no_wait_while_the_agent_or_moor_has_the_gate_or_after_a_no_ended_it() {
    let g = Gates::parse(&[doc("proof", &[], &[], 1)], &[]).unwrap();
    let no = GateLine::Answered {
        gate: id("proof"),
        try_n: 1,
        by: "me".to_owned(),
        answer: Answer::No,
        note: String::new(),
    };
    assert_eq!(wait(&g, &[no]), None);
    let g = Gates::parse(&[doc("proof", &["proof.md"], &[], 1)], &[]).unwrap();
    assert_eq!(wait(&g, &[]), None);
}

#[test]
fn a_reopened_gate_forgets_the_failure_before_the_reopening() {
    let g = Gates::parse(&[doc("proof", &[], &["make"], 1)], &[]).unwrap();
    let reopened = GateLine::Reopened {
        gate: id("proof"),
        by: "me".to_owned(),
    };
    let lines = [failed_check(1), reopened, failed_check(1)];
    let w = wait(&g, &lines).unwrap();
    assert_eq!(w.seq, 3);
    assert_eq!(w.command.as_deref(), Some("make"));
}

#[test]
fn the_card_json_is_pinned() {
    let g = Gates::parse(&[doc("proof", &[], &["make"], 1)], &[]).unwrap();
    let json = serde_json::to_value(wait(&g, &[failed_check(1)]).unwrap()).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "gate": "proof",
            "kind": "stuck",
            "seq": 1,
            "try_n": null,
            "why": "out of tries: check 1 (`make`) exited 101",
            "handed": [],
            "note": null,
            "command": "make",
            "tail": "1 test failed\n",
            "missing": [],
            "hint": "Send the agent a message to reopen the gate with fresh tries."
        })
    );
}

fn busy(try_n: u8) -> GateLine {
    GateLine::NoAnswer {
        gate: id("proof"),
        try_n,
        check: crate::gate::CheckOf::Gate { check: 0 },
        why: crate::gate::Unanswered::NotRun {
            reason: "Box 3 is busy".to_owned(),
        },
    }
}

#[test]
fn a_gate_moor_could_not_check_carries_the_command_and_says_moor_failed() {
    let g = Gates::parse(&[doc("proof", &[], &["make"], 2)], &[]).unwrap();
    assert_eq!(wait(&g, &[busy(1)]), None, "once is tried again");
    let w = wait(&g, &[busy(1), busy(1)]).unwrap();
    assert_eq!(w.kind, WaitKind::Stuck);
    assert_eq!(w.seq, 2);
    assert_eq!(w.command.as_deref(), Some("make"));
    let why = w.why.as_deref().unwrap();
    assert!(why.contains("Box 3 is busy"), "{why}");
    assert!(w.hint.contains("not the agent"), "{}", w.hint);
    assert!(w.hint.contains("close the Box"), "{}", w.hint);
    assert_eq!(w.tail, None);
}

#[test]
fn the_closing_stage_names_the_outcome_check_moor_could_not_run() {
    let merged = crate::gate::OutcomeDoc {
        name: "merged".to_owned(),
        says: None,
        tone: None,
        checks: vec![CheckDoc::Said(crate::gate::SaidDoc {
            run: "gh pr view".to_owned(),
            says: "The pull request is merged.".to_owned(),
        })],
        anytime: false,
    };
    let g = Gates::parse(&[doc("proof", &[], &[], 1)], &[merged]).unwrap();
    let yes = GateLine::Answered {
        gate: id("proof"),
        try_n: 1,
        by: "me".to_owned(),
        answer: Answer::Yes,
        note: String::new(),
    };
    let busy = GateLine::NoAnswer {
        gate: crate::gate::closing(),
        try_n: 1,
        check: crate::gate::CheckOf::Outcome {
            outcome: crate::gate::OutcomeName("merged".to_owned()),
            check: 0,
        },
        why: crate::gate::Unanswered::Lost,
    };
    let w = wait(&g, &[yes, busy.clone(), busy]).unwrap();
    assert_eq!(w.gate, "outcome");
    assert_eq!(w.command.as_deref(), Some("gh pr view"));
    assert_eq!(w.says.as_deref(), Some("The pull request is merged."));
    assert!(w.why.as_deref().unwrap().contains("lost"), "{:?}", w.why);
    assert!(w.hint.contains("not the agent"), "{}", w.hint);
}

#[test]
fn a_lone_no_answer_before_the_agent_went_quiet_is_not_the_cause() {
    let g = Gates::parse(&[doc("proof", &["proof.md"], &["make"], 1)], &[]).unwrap();
    let lines = [
        busy(1),
        GateLine::Reminded {
            gate: id("proof"),
            try_n: 1,
        },
        GateLine::Quiet {
            gate: id("proof"),
            try_n: 1,
        },
    ];
    let w = wait(&g, &lines).unwrap();
    assert_eq!(w.why.as_deref(), Some("the agent went quiet"));
    assert_eq!(w.command, None);
}
