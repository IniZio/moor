//! Tests for the `show` text form.

use moor_core::Response;
use serde_json::{json, Value};

use super::super::render;

fn row(name: &str, with_box: bool) -> Value {
    json!({
        "id": 1, "name": name, "repo": "/r", "lifecycle": "ready", "session_state": "idle",
        "branch": "moor/x",
        "driver": if with_box { json!({
            "state": "running", "provider": "local", "workdir": "/work",
            "egress": "none", "sleep": {}
        }) } else { Value::Null },
        "agent": {"state": "working", "pane": "live"},
        "updated_at": "t"
    })
}

fn show(v: Value) -> String {
    render(&Response::ok("show", Some(v)), false).0
}

#[test]
fn empty_list_says_no_boxes() {
    assert_eq!(show(json!({"boxes": []})), "no Boxes\n");
}

#[test]
fn list_one_box() {
    let out = show(json!({"boxes": [row("alpha", true)]}));
    assert_eq!(out, "alpha  ready  idle  working  running  /work\n");
}

#[test]
fn list_two_boxes_aligns_columns_and_dashes_missing_box() {
    let out = show(json!({"boxes": [row("a", true), row("longer", false)]}));
    assert_eq!(
        out,
        "a       ready  idle  working  running  /work\nlonger  ready  idle  working  -        -\n"
    );
}

#[test]
fn detail_with_box() {
    let out = show(json!({"box": row("alpha", true)}));
    assert_eq!(
        out,
        "name:     alpha\nlifecycle: ready\nsession:   idle\nrepo:     /r\nbranch:   moor/x\n\
         box:      local running workdir=/work egress=none\nagent:    working pane=live\n"
    );
}

#[test]
fn detail_lists_every_repo_of_a_multi_repo_box() {
    let mut t = row("alpha", true);
    let fields = t.as_object_mut().unwrap();
    fields.insert("repo".into(), json!("o/a"));
    fields.insert("other_repos".into(), json!(["o/b", "o/c"]));
    let out = show(json!({"box": t}));
    assert!(out.contains("\nrepo:     o/a, o/b, o/c\n"), "{out}");
}

#[test]
fn detail_shows_the_delivered_cpus_and_memory_after_the_box_line() {
    let mut t = row("alpha", true);
    let driver = t
        .get_mut("driver")
        .and_then(Value::as_object_mut)
        .expect("driver is an object");
    driver.insert("cpus".into(), json!(4));
    driver.insert("mem_mib".into(), json!(7900));
    let out = show(json!({"box": t}));
    assert!(
        out.contains("egress=none\nmachine:  4 vCPU, 7900 MiB\nagent:"),
        "{out}"
    );
}

#[test]
fn detail_without_a_read_shape_has_no_machine_line() {
    let out = show(json!({"box": row("alpha", true)}));
    assert!(!out.contains("machine:"), "{out}");
}

#[test]
fn detail_lists_the_question_and_numbered_options() {
    let mut t = row("alpha", false);
    t.as_object_mut().expect("row is an object").insert(
        "agent".into(),
        json!({
            "state": "needs_input", "pane": "live",
            "question": "Do you want to proceed?",
            "options": ["Yes", "No, and tell Claude what to do differently (esc)"]
        }),
    );
    let out = show(json!({"box": t}));
    assert!(out.contains(
        "agent:    needs_input pane=live\n  question: Do you want to proceed?\n  1. Yes\n  \
         2. No, and tell Claude what to do differently (esc)\n"
    ));
}

#[test]
fn detail_without_box() {
    let out = show(json!({"box": row("alpha", false)}));
    assert!(out.contains("box:      none\n"));
}

#[test]
fn detail_with_events_setup_and_pane() {
    let out = show(json!({
        "box": row("alpha", true),
        "setup": {"ran": true, "ok": true},
        "dotfiles": {"ran": true, "ok": false},
        "events": [{"id": 1, "kind": "started", "payload": {}, "ts": "2026-01-01T00:00:00Z"}],
        "pane": {"tail": "line1\nline2", "status": "live"}
    }));
    assert!(out.contains("setup: ok\ndotfiles: failed\n"));
    assert!(out.contains("events:\n  2026-01-01T00:00:00Z  started\n"));
    assert!(out.ends_with("--- pane ---\nline1\nline2\n"));
}

#[test]
fn json_form_keeps_the_raw_value() {
    let v = json!({"boxes": [row("alpha", true)]});
    let out = crate::cli::render_json::render(&Response::ok("show", Some(v.clone())));
    let parsed: Value = serde_json::from_str(&out).expect("json");
    assert_eq!(parsed.get("value"), Some(&v));
}

#[test]
fn warnings_still_go_to_stderr() {
    let mut resp = Response::ok("show", Some(json!({"boxes": []})));
    resp.warnings.push(moor_core::Warning {
        code: "w".into(),
        message: "careful".into(),
    });
    let (out, err) = render(&resp, false);
    assert_eq!(out, "no Boxes\n");
    assert_eq!(err, "moor: warning: careful\n");
}

fn with_attention(mut t: Value) -> Value {
    t.as_object_mut().unwrap().insert(
        "attention".into(),
        json!([{
            "kind": "agent_rate_limited", "rule": "claude", "status": 429,
            "message": "Claude account rate limited (429)"
        }]),
    );
    t
}

#[test]
fn list_prints_a_needs_attention_line_under_the_row() {
    let out = show(json!({"boxes": [with_attention(row("alpha", true)), row("b", true)]}));
    assert_eq!(
        out,
        "alpha  ready  idle  working  running  /work\n  needs attention: Claude account rate limited (429)\nb      ready  idle  working  running  /work\n"
    );
}

#[test]
fn detail_prints_a_needs_attention_line() {
    let out = show(json!({"box": with_attention(row("alpha", true))}));
    assert!(out.contains(
        "agent:    working pane=live\nneeds attention: Claude account rate limited (429)\n"
    ));
}

#[test]
fn detail_omits_the_line_without_attention() {
    let out = show(json!({"box": row("alpha", true)}));
    assert!(!out.contains("needs attention"));
}

#[test]
fn show_session_renders_host_with_child() {
    let v = json!({
        "session": 5, "box": null, "ordinal": 1, "parent": null, "workdir": "/w",
        "agent": "claude", "agent_session_id": "abc", "state": "working",
        "started_at": "t", "ended_at": null,
        "children": [{"session": 7, "box": "box-name", "state": "working"}]
    });
    let out = render(&Response::ok("show-session", Some(v)), false).0;
    assert_eq!(
        out,
        "Session 5 working (host) in /w\nagent session abc\n  child 7 box-name working\n"
    );
}

#[test]
fn detail_names_the_held_machine_its_slot_and_ports() {
    let mut t = row("alpha", true);
    let held = json!({"name": "mini", "slot": 1, "ports": [8091, 8095]});
    t.as_object_mut()
        .unwrap()
        .insert("held_machine".into(), held);
    let out = show(json!({"box": t}));
    assert!(
        out.contains("egress=none\nattached: mini slot 1 ports 8091,8095\nagent:"),
        "{out}"
    );
}

#[test]
fn detail_says_where_a_waiting_machine_claim_stands_in_line() {
    for (n, line) in [
        (1, "1st"),
        (2, "2nd"),
        (3, "3rd"),
        (4, "4th"),
        (11, "11th"),
        (22, "22nd"),
    ] {
        let mut t = row("alpha", true);
        let held =
            json!({"name": "mini", "slot": 0, "ports": [], "state": "waiting", "position": n});
        t.as_object_mut()
            .unwrap()
            .insert("held_machine".into(), held);
        let out = show(json!({"box": t}));
        assert!(
            out.contains(&format!("machine:  mini waiting, {line} in line\n")),
            "{out}"
        );
    }
}

fn unfinished(mut t: Value) -> Value {
    if let Some(fields) = t.as_object_mut() {
        fields.insert("lifecycle".into(), json!("asleep"));
        fields.insert("unfinished_turn".into(), json!(true));
    }
    t
}

#[test]
fn list_and_detail_say_when_a_box_slept_mid_turn() {
    let out = show(json!({"boxes": [unfinished(row("alpha", true)), row("b", true)]}));
    assert!(
        out.contains("/work\n  asleep, turn unfinished\nb "),
        "{out}"
    );
    let out = show(json!({"box": unfinished(row("alpha", true))}));
    assert!(
        out.contains("session:   idle\nasleep, turn unfinished\nrepo:"),
        "{out}"
    );
    assert!(!show(json!({"box": row("alpha", true)})).contains("unfinished"));
}

fn with_gate(mut t: Value, progress: Value, status: &Value) -> Value {
    let fields = t.as_object_mut().unwrap();
    fields.insert("progress".into(), progress);
    fields.insert(
        "gate".into(),
        json!({"gates": ["proof"], "status": status, "wait": null}),
    );
    t
}

fn asks() -> Value {
    with_gate(
        row("alpha", true),
        json!({"state": "needs_you", "line": "gate proof: waiting for your approval"}),
        &json!({"state": "open", "gate": "proof", "try_n": 1, "next": "ask"}),
    )
}

#[test]
fn list_shows_the_progress_state_and_the_gate_line_under_the_row() {
    let out = show(json!({"boxes": [asks(), row("b", true)]}));
    assert_eq!(
        out,
        "alpha  ready  idle  working  running  /work\n  needs you: gate proof: waiting for your approval\nb      ready  idle  working  running  /work\n"
    );
}

#[test]
fn detail_shows_the_progress_state_and_the_gate_line() {
    let out = show(json!({"box": asks()}));
    assert!(
        out.contains(
            "session:   idle\nprogress: needs you: gate proof: waiting for your approval\nrepo:"
        ),
        "{out}"
    );
}

#[test]
fn a_working_box_with_an_open_gate_shows_the_gate_beside_what_it_does() {
    let working = with_gate(
        row("alpha", true),
        json!({"state": "working", "line": "editing main.rs"}),
        &json!({"state": "open", "gate": "proof", "try_n": 2, "next": "agent"}),
    );
    let out = show(json!({"boxes": [working]}));
    assert!(
        out.ends_with(
            "  working: editing main.rs\n  gate proof, try 2: waiting for the agent's evidence\n"
        ),
        "{out}"
    );
}

#[test]
fn a_stuck_failed_and_passed_gate_each_say_so() {
    for (status, want) in [
        (
            json!({"state": "stuck", "gate": "proof", "why": "out of tries: check 1 (`make`) exited 2"}),
            "gate proof is stuck: out of tries: check 1 (`make`) exited 2",
        ),
        (
            json!({"state": "failed", "gate": "proof", "why": "the owner said no: redo"}),
            "gate proof failed: the owner said no: redo",
        ),
        (json!({"state": "passed"}), "gates: all passed"),
    ] {
        let t = with_gate(
            row("alpha", true),
            json!({"state": "done", "line": "PR #4"}),
            &status,
        );
        let out = show(json!({"boxes": [t]}));
        assert!(out.contains(&format!("  done: PR #4\n  {want}\n")), "{out}");
    }
}

#[test]
fn a_real_serialised_gate_and_progress_render_as_the_fronts_get_them() {
    use moor_core::gate::{GateDoc, Gates};
    use moor_core::progress::{Progress, ProgressView};
    use moor_core::view::gate::GateView;

    let doc = GateDoc {
        name: "proof".to_owned(),
        evidence: Vec::new(),
        checks: Vec::new(),
        approval: Some(moor_core::gate::Approval::Owner),
        max_tries: None,
    };
    let gate = GateView::of(&Gates::parse(&[doc], &[]).unwrap(), &[]);
    let progress = ProgressView {
        title: "t".to_owned(),
        state: Progress::NeedsYou,
        failed: false,
        stale: false,
        line: "gate proof: waiting for your approval".to_owned(),
        last_event_at: None,
        pr: None,
        needs: None,
    };
    let mut t = row("alpha", true);
    let fields = t.as_object_mut().unwrap();
    fields.insert("progress".into(), serde_json::to_value(progress).unwrap());
    fields.insert("gate".into(), serde_json::to_value(gate).unwrap());
    let list = show(json!({"boxes": [t.clone()]}));
    assert!(
        list.contains("  needs you: gate proof: waiting for your approval\n"),
        "{list}"
    );
    assert!(show(json!({"box": t})).contains("progress: needs you: gate proof:"));
}

/// A stuck gate's card as the fronts get it: the `needs` of a progress whose gate waits for a steer.
fn stuck_box(lines: &[moor_core::gate::GateLine], doc: moor_core::gate::GateDoc) -> Value {
    use moor_core::gate::{wait, Gates};
    let gates = Gates::parse(&[doc], &[]).unwrap();
    let waits = wait(&gates, lines).unwrap();
    let why = waits.why.clone().unwrap();
    let card = serde_json::to_value(waits).unwrap();
    with_gate(
        row("alpha", true),
        json!({
            "state": "needs_you",
            "line": format!("gate proof is stuck: {why}"),
            "needs": {"kind": "stuck", "gate": card},
        }),
        &json!({"state": "stuck", "gate": "proof", "why": why}),
    )
}

fn proof_doc(evidence: &[&str], checks: &[&str]) -> moor_core::gate::GateDoc {
    moor_core::gate::GateDoc {
        name: "proof".to_owned(),
        evidence: evidence.iter().map(|s| (*s).to_owned()).collect(),
        checks: checks
            .iter()
            .map(|s| moor_core::gate::CheckDoc::from(*s))
            .collect(),
        approval: None,
        max_tries: Some(1),
    }
}

#[test]
fn a_stuck_box_shows_the_failing_command_its_output_and_the_hint_under_the_reason() {
    use moor_core::gate::GateLine;
    use moor_core::workflow::StepId;
    let lines = [GateLine::Checked {
        gate: StepId::parse("proof").unwrap(),
        try_n: 1,
        check: 0,
        exit: 2,
        tail: "boom\nline two\n".to_owned(),
    }];
    let t = stuck_box(&lines, proof_doc(&[], &["make test"]));
    let want =
        "  needs you: gate proof is stuck: out of tries: check 1 (`make test`) exited 2\n    \
         command: make test\n    output:\n      boom\n      line two\n    \
         hint: Send the agent a message to reopen the gate with fresh tries.\n";
    let out = show(json!({"boxes": [t]}));
    assert!(out.ends_with(want), "{out}");
    let out = show(json!({"box": t}));
    assert!(
        out.contains("\n            command: make test\n            output:\n              boom\n"),
        "{out}"
    );
}

#[test]
fn a_stuck_box_with_a_sentence_says_what_is_not_yet_true_before_the_command() {
    use moor_core::gate::{CheckDoc, GateLine, SaidDoc};
    use moor_core::workflow::StepId;
    let lines = [GateLine::Checked {
        gate: StepId::parse("proof").unwrap(),
        try_n: 1,
        check: 0,
        exit: 1,
        tail: "no never.txt\n".to_owned(),
    }];
    let mut doc = proof_doc(&[], &[]);
    doc.checks = vec![CheckDoc::Said(SaidDoc {
        run: "test -f never.txt".to_owned(),
        says: "The file never.txt exists.".to_owned(),
    })];
    let out = show(json!({"boxes": [stuck_box(&lines, doc)]}));
    assert!(
        out.contains(
            "\n    not yet true: The file never.txt exists.\n    command: test -f never.txt\n    output:\n      no never.txt\n"
        ),
        "{out}"
    );
}

#[test]
fn a_stuck_box_with_missing_evidence_names_it_under_the_reason() {
    use moor_core::gate::GateLine;
    use moor_core::workflow::StepId;
    let refused = || GateLine::Refused {
        gate: StepId::parse("proof").unwrap(),
        try_n: 1,
        missing: vec!["proof.md".to_owned(), "notes.md".to_owned()],
    };
    let t = stuck_box(
        &[refused(), refused()],
        proof_doc(&["proof.md", "notes.md"], &[]),
    );
    let out = show(json!({"boxes": [t]}));
    assert!(
        out.contains("\n    missing: proof.md, notes.md\n    hint: Send the agent a message"),
        "{out}"
    );
    assert!(
        !out.contains("command:") && !out.contains("output:"),
        "{out}"
    );
}

/// A Box whose gates are `names`, `passed` of which passed before the run ended in `outcome`.
fn ended_box(names: &[&str], passed: usize, outcome: &Value) -> Value {
    let mut t = with_gate(
        row("alpha", true),
        json!({"state": "done", "line": "nothing to do"}),
        &json!({"state": "passed", "outcome": outcome}),
    );
    let gate = t
        .pointer_mut("/gate")
        .and_then(Value::as_object_mut)
        .unwrap();
    gate.insert("gates".into(), json!(names));
    gate.insert("gates_passed".into(), json!(passed));
    t
}

#[test]
fn an_anytime_outcome_that_ended_the_run_says_how_many_gates_passed_not_all_passed() {
    let t = ended_box(
        &["proof", "done"],
        0,
        &json!({"name": "nothing to do", "tone": "plain"}),
    );
    let out = show(json!({"boxes": [t]}));
    assert!(
        out.contains("  outcome: nothing to do, 0 of 2 gates passed\n"),
        "{out}"
    );
    assert!(!out.contains("all passed"), "{out}");
}

#[test]
fn a_named_outcome_after_every_gate_says_so_and_the_implicit_done_reads_as_before() {
    let named = ended_box(
        &["proof", "done"],
        2,
        &json!({"name": "check failed", "tone": "bad"}),
    );
    let out = show(json!({"boxes": [named]}));
    assert!(
        out.contains("  gates: all passed, outcome: check failed\n"),
        "{out}"
    );
    let implicit = ended_box(&["proof"], 1, &json!({"name": "done", "tone": "good"}));
    let out = show(json!({"boxes": [implicit]}));
    assert!(out.contains("  gates: all passed\n"), "{out}");
    assert!(!out.contains("outcome:"), "{out}");
}

#[test]
fn a_box_stuck_at_the_closing_stage_lists_each_outcome_tried() {
    let card = json!({
        "gate": "outcome", "kind": "stuck", "seq": 1, "why": "out of tries: no outcome held",
        "tail": "one blob of everything", "hint": "Send the agent a message to reopen the gate.",
        "tried": [
            {"outcome": "check passed", "says": "The newest status is success.", "command": "test \"$s\" = success", "exit": 1, "tail": "status is none\n"},
            {"outcome": "check failed", "command": "false", "exit": null, "tail": ""},
        ],
    });
    let t = with_gate(
        row("alpha", true),
        json!({
            "state": "needs_you", "line": "gate outcome is stuck: out of tries: no outcome held",
            "needs": {"kind": "stuck", "gate": card},
        }),
        &json!({"state": "stuck", "gate": "outcome", "why": "out of tries: no outcome held"}),
    );
    let out = show(json!({"boxes": [t]}));
    assert!(
        out.contains(
            "    not met: The newest status is success.\n      command: test \"$s\" = success\n      exit: 1\n      output:\n        status is none\n"
        ),
        "{out}"
    );
    assert!(
        out.contains("    not met: check failed\n      command: false\n      exit: output lost\n"),
        "{out}"
    );
    assert!(!out.contains("one blob"), "{out}");
    assert!(out.contains("    hint: Send the agent a message"), "{out}");
}
