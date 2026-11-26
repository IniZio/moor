//! What the agent is told about its gates.

use super::*;
use crate::artifact::ArtifactFile;
use crate::decide::workflow::evidence_gate;
use crate::workflow::{StepId, StepReport, StepResult};

fn doc(name: &str, evidence: &[&str], checks: &[&str], approval: Option<Approval>) -> GateDoc {
    GateDoc {
        name: name.to_owned(),
        evidence: evidence.iter().map(|s| (*s).to_owned()).collect(),
        checks: checks.iter().map(|s| CheckDoc::from(*s)).collect(),
        approval,
        max_tries: None,
    }
}

fn gates() -> Gates {
    Gates::parse(
        &[
            doc(
                "proof",
                &["proof.md", "shots-*"],
                &["make test", "make lint"],
                None,
            ),
            doc("tests", &[], &["make test"], Some(Approval::Owner)),
        ],
        &[],
    )
    .unwrap()
}

#[test]
fn the_first_message_says_how_to_reach_report_step_in_words_any_agent_follows() {
    let text = gates().first_message("Fix the typo.");
    assert!(
        text.contains(
            "report_step is a tool of the moor MCP server; load it first if your tools are \
             loaded on demand."
        ),
        "{text}"
    );
    assert!(
        !text.contains("ToolSearch"),
        "no one harness's tool: {text}"
    );
}

#[test]
fn the_first_message_asks_for_each_outside_write_with_report_output() {
    let text = gates().first_message("Fix the typo.");
    assert!(
        text.contains(
            "After each write outside moor, such as a pull request comment, call \
             report_output with its kind, URL and title."
        ),
        "{text}"
    );
}

#[test]
fn the_first_message_lists_each_gate_with_its_evidence_and_who_runs_what() {
    let text = gates().first_message("Fix the typo.\n");
    assert!(
        text.starts_with("Fix the typo.\n\nmoor keeps this Box open"),
        "{text}"
    );
    assert!(text.contains("report_step"), "{text}");
    assert!(
        text.contains("Gate `proof`: leave proof.md, shots-* in .moor/artifacts/"),
        "{text}"
    );
    assert!(text.contains("step \"proof\", result ok"), "{text}");
    assert!(text.contains("moor runs its 2 checks itself"), "{text}");
    assert!(text.contains("Gate `tests`: you report nothing"), "{text}");
    assert!(text.contains("moor runs its check itself"), "{text}");
    assert!(text.contains("asks the Box's owner to approve"), "{text}");
}

#[test]
fn the_first_message_says_the_gates_go_in_order_and_that_moor_says_when_one_is_in_hand() {
    let text = gates().first_message("Fix the typo.\n");
    assert!(text.contains("in this order, one at a time"), "{text}");
    assert!(
        text.contains("moor tells you when each gate is in hand"),
        "{text}"
    );
    assert!(
        text.contains("Report only the gate that is in hand"),
        "{text}"
    );
}

#[test]
fn a_report_is_asked_for_a_one_line_note_of_what_was_done() {
    let g = gates();
    let brief = g.get(0).unwrap().brief();
    assert!(brief.contains("a one-line note of what you did"), "{brief}");
    let remind = g.get(0).unwrap().remind_steer();
    assert!(
        remind.contains("a one-line note of what you did"),
        "{remind}"
    );
}

#[test]
fn the_hand_over_names_the_gate_that_passed_and_the_one_in_hand_with_its_brief() {
    let g = gates();
    let text = g
        .get(1)
        .unwrap()
        .handoff_steer(&StepId::parse("proof").unwrap());
    assert!(text.contains("Gate `proof` has passed"), "{text}");
    assert!(text.contains("Gate `tests` is now in hand"), "{text}");
    assert!(text.contains(&g.get(1).unwrap().brief()), "{text}");
}

#[test]
fn the_persons_words_come_back_out_of_the_first_message() {
    let text = gates().first_message("Fix the typo.\n");
    assert_eq!(split_gate_preamble(&text), Some("Fix the typo."));
    assert_eq!(split_gate_preamble("Fix the typo."), None);
}

#[test]
fn the_reminder_names_the_gate_and_its_evidence() {
    let g = gates();
    let text = g.get(0).unwrap().remind_steer();
    assert!(text.contains("Gate `proof`"), "{text}");
    assert!(text.contains("proof.md, shots-*"), "{text}");
    assert!(text.contains("report_step"), "{text}");
}

#[test]
fn a_failed_check_sends_the_agent_back_to_report_only_when_the_gate_takes_a_report() {
    let g = gates();
    let with_report = g
        .get(0)
        .unwrap()
        .failed_check_steer(1, ("exited 2", "boom\n"));
    assert!(
        with_report.contains("check 2 (`make lint`) exited 2"),
        "{with_report}"
    );
    assert!(with_report.contains("boom"), "{with_report}");
    assert!(
        with_report.contains("call report_step again"),
        "{with_report}"
    );
    assert!(!with_report.contains("once you are quiet"), "{with_report}");
    let without = g.get(1).unwrap().failed_check_steer(0, ("exited 1", ""));
    assert!(
        without.contains("moor runs the checks again once you are quiet"),
        "{without}"
    );
    assert!(!without.contains("report_step"), "{without}");
}

fn file(name: &str, size: u64) -> ArtifactFile {
    ArtifactFile {
        name: name.to_owned(),
        size,
        sha256: "a".repeat(64),
    }
}

fn report(result: StepResult, note: &str) -> StepReport {
    StepReport {
        step: StepId::parse("proof").unwrap(),
        result,
        evidence: vec!["proof.md".to_owned()],
        note: note.to_owned(),
    }
}

#[test]
fn a_gates_evidence_is_checked_like_a_steps() {
    let g = gates();
    let ev = g.get(0).unwrap().evidence.as_ref();
    let ok = report(StepResult::Ok, "");
    let all = [file("proof.md", 5), file("shots-1.png", 9)];
    assert_eq!(evidence_gate(ev, &ok, &all), Ok(()));
    let problems = evidence_gate(ev, &ok, &[file("proof.md", 0)]).unwrap_err();
    assert_eq!(
        problems,
        [
            "`proof.md` is empty",
            "no file matches `shots-*` in .moor/artifacts/"
        ]
    );
    assert_eq!(
        evidence_gate(ev, &report(StepResult::Fail, "no luck"), &[]),
        Ok(())
    );
    let bare = evidence_gate(ev, &report(StepResult::Fail, " "), &[]).unwrap_err();
    assert_eq!(bare, ["a fail report needs a note that says why"]);
    assert_eq!(evidence_gate(None, &ok, &[]), Ok(()));
}

#[test]
fn the_owners_note_splits_out_of_a_no_whatever_it_holds() {
    let gate = &gates().iter().next().unwrap().clone();
    let note = "use the blue header\n\nFix it, then rewrite it";
    let text = gate.owner_no_steer(note);
    let (rest, said) = split_owner_note(&text).unwrap();
    assert_eq!(said, note);
    assert!(
        rest.starts_with("The owner did not approve gate `proof`."),
        "{rest}"
    );
    assert!(
        rest.contains("\n\nFix it, then call report_step again"),
        "{rest}"
    );
    assert!(!rest.contains("blue header"), "{rest}");

    assert_eq!(
        split_owner_note(&gate.owner_no_steer("  ")),
        None,
        "no note, nothing to split"
    );
    assert_eq!(
        split_owner_note("The owner wrote: hi\n\nFix it, then go"),
        None
    );
}

#[test]
fn the_preamble_names_the_outcomes_and_says_moor_decides_them() {
    let outcomes: Vec<OutcomeDoc> = serde_json::from_value(serde_json::json!([
        {"name": "nothing to do", "says": "The pull request is closed or moved.", "tone": "plain",
         "checks": [{"run": "true", "says": "The pull request is not open."}], "anytime": true},
        {"name": "check passed", "checks": ["true"]}
    ]))
    .unwrap();
    let with = Gates::parse(&gates().docs(), &outcomes).unwrap();
    let first = with.first_message("Collect evidence.");
    assert!(
        first.contains("not yours to say: moor decides it"),
        "{first}"
    );
    assert!(first.contains("Never fake evidence"), "{first}");
    assert!(
        first.contains("- `nothing to do`: The pull request is closed or moved. (anytime) It holds when: “The pull request is not open.”"),
        "{first}"
    );
    assert!(first.contains("- `check passed`"), "{first}");
    let plain = gates().first_message("Collect evidence.");
    assert!(
        !plain.contains("outcome"),
        "no outcomes, no word of them: {plain}"
    );
}
