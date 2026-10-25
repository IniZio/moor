#![allow(clippy::indexing_slicing, reason = "test code")]

use moor_core::name::BoxName;
use serde_json::json;

use super::*;
use crate::db::SlackBox;

/// The id of the Box the tests follow.
const ID: BoxId = BoxId::new(7);

/// Box `fix-bug` on `repo` in `state`, with status message `ts` showing `text`.
fn fed(state: RowState, ts: Option<&str>, text: Option<&str>) -> FedBox {
    FedBox {
        boxed: SlackBox {
            channel: "C1".into(),
            thread_ts: "1.0".into(),
            box_name: BoxName::parse_allow_bare("fix-bug").unwrap(),
            repo: "acme/app".into(),
            goal: "fix it".into(),
            base: None,
            requested_by: "U1".into(),
            parent: None,
            state,
        },
        status_ts: ts.map(str::to_owned),
        status_text: text.map(str::to_owned),
    }
}

/// A bound Box `fix-bug` with branch `moor/fix-bug` and status message `ts` showing `text`.
fn bound(ts: Option<&str>, text: Option<&str>) -> FedBox {
    let branch = Some("moor/fix-bug".to_owned());
    fed(RowState::Bound { box_id: ID, branch }, ts, text)
}

fn views(view: View) -> HashMap<BoxId, View> {
    HashMap::from([(ID, view)])
}

fn folded(events: &[(&str, Value)]) -> View {
    let mut v = View::default();
    for (kind, payload) in events {
        v.fold(kind, payload, 0);
    }
    v
}

/// The only step planned, as `(ts, text)`.
fn one(steps: &[Step]) -> (Option<&str>, &str) {
    assert_eq!(steps.len(), 1, "{steps:?}");
    let step = steps.first().unwrap();
    (step.ts.as_deref(), step.text.as_str())
}

#[test]
fn needs_input_shows_question() {
    let v = folded(&[
        ("working", json!({})),
        (
            "needs_input",
            json!({"question": "Which db?", "options": []}),
        ),
    ]);
    assert!(render("fix-bug", &v, 0).contains("waiting on a question (<1m): Which db?"));
    let v = folded(&[
        ("needs_input", json!({"question": "x"})),
        ("busy", json!({})),
    ]);
    assert_eq!(v.state, State::Working);
}

#[test]
fn closed_and_box_states_fold() {
    let v = folded(&[
        ("box_requested", json!({})),
        ("box_ready", json!({})),
        ("box_asleep", json!({})),
        ("closed", json!({"from": "idle"})),
    ]);
    let text = render("fix-bug", &v, 0);
    assert!(text.contains("closed"), "{text}");
    assert!(text.contains("Box: asleep"), "{text}");
    assert_eq!(
        folded(&[("box_gone", json!({}))]).machine,
        Some(Machine::Gone)
    );
}

#[test]
fn question_cannot_ping() {
    let v = folded(&[("needs_input", json!({"question": "<!channel> hi"}))]);
    assert!(!render("t", &v, 0).contains("<!channel>"));
}

#[test]
fn a_question_is_cut_after_300_characters() {
    let render_question = |q: &str| {
        let v = folded(&[("needs_input", json!({ "question": q }))]);
        render("t", &v, 0)
    };
    let fits = "é".repeat(300);
    assert_eq!(
        render_question(&fits),
        format!("*t*: waiting on a question (<1m): {fits}")
    );
    let long = "é".repeat(301);
    assert_eq!(
        render_question(&long),
        format!("*t*: waiting on a question (<1m): {fits}...")
    );
}

#[test]
fn the_reason_and_the_setup_line_cannot_ping() {
    let raw = "<!channel> & <@U1>";
    let shown = "&lt;!channel&gt; &amp; &lt;@U1&gt;";
    for (kind, key, head) in [
        ("rejected", "reason", "rejected"),
        ("setup_failed", "line", "setup failed"),
    ] {
        let v = folded(&[(kind, json!({ key: raw }))]);
        assert_eq!(render("t", &v, 0), format!("*t*: {head}: {shown}"));
    }
}

#[test]
fn the_status_names_no_pull_request() {
    let text = render("fix-bug", &View::default(), 0);
    assert_eq!(text, "*fix-bug*: starting (<1m)");
}

#[test]
fn posts_once_then_updates() {
    let t0 = crate::feed::now();
    let mut p = Planner::default();
    let mut vs = views(View::default());
    let steps = p.plan(&[bound(None, None)], &vs, t0, 0);
    let (ts, text) = one(&steps);
    assert_eq!(ts, None);
    let first = text.to_owned();
    p.record(steps.first().unwrap(), t0);

    let later = t0 + UPDATE_EVERY;
    assert!(p
        .plan(&[bound(Some("9.9"), Some(&first))], &vs, later, 0)
        .is_empty());

    vs.get_mut(&ID).unwrap().fold("working", &json!({}), 0);
    let steps = p.plan(&[bound(Some("9.9"), Some(&first))], &vs, later, 0);
    let (ts, text) = one(&steps);
    assert_eq!(ts, Some("9.9"));
    assert!(text.contains("working"), "{text}");
}

#[test]
fn quick_changes_coalesce_into_one_later_update() {
    let t0 = crate::feed::now();
    let mut p = Planner::default();
    let mut vs = views(View::default());
    let first = p.plan(&[bound(Some("9.9"), None)], &vs, t0, 0);
    p.record(first.first().unwrap(), t0);
    let shown = first.first().unwrap().text.clone();
    let rows = [bound(Some("9.9"), Some(&shown))];

    vs.get_mut(&ID).unwrap().fold("working", &json!({}), 0);
    let soon = t0 + Duration::from_millis(500);
    assert!(p.plan(&rows, &vs, soon, 0).is_empty());
    vs.get_mut(&ID).unwrap().fold("idled", &json!({}), 0);
    assert!(p.plan(&rows, &vs, soon, 0).is_empty());

    let due = t0 + UPDATE_EVERY;
    let steps = p.plan(&rows, &vs, due, 0);
    let (_, text) = one(&steps);
    assert!(text.contains("idle"), "{text}");
    let shown = text.to_owned();
    p.record(steps.first().unwrap(), due);
    let rows = [bound(Some("9.9"), Some(&shown))];
    assert!(p.plan(&rows, &vs, due + UPDATE_EVERY, 0).is_empty());
}

#[test]
fn failed_send_stays_due() {
    let t0 = crate::feed::now();
    let mut p = Planner::default();
    let vs = views(View::default());
    let rows = [bound(None, None)];
    let steps = p.plan(&rows, &vs, t0, 0);
    p.record(steps.first().unwrap(), t0);
    assert!(p
        .plan(&rows, &vs, t0 + Duration::from_millis(1), 0)
        .is_empty());
    assert_eq!(p.plan(&rows, &vs, t0 + UPDATE_EVERY, 0).len(), 1);
}

#[test]
fn a_gone_box_gets_its_final_status_in_its_message_and_none_without_one() {
    let t0 = crate::feed::now();
    let p = Planner::default();
    let vs = views(folded(&[("working", json!({}))]));
    let gone = |ts: Option<&str>| {
        fed(
            RowState::Gone {
                box_id: ID,
                branch: None,
            },
            ts,
            Some("*fix-bug*: working"),
        )
    };
    assert!(
        p.plan(&[gone(None)], &vs, t0, 0).is_empty(),
        "no new message"
    );
    let steps = p.plan(&[gone(Some("9.9"))], &vs, t0, 0);
    assert_eq!(one(&steps), (Some("9.9"), "*fix-bug*: closed"));
}

#[test]
fn each_box_follows_only_the_events_of_its_own_id() {
    let t0 = crate::feed::now();
    let p = Planner::default();
    let vs = HashMap::from([(BoxId::new(8), folded(&[("working", json!({}))]))]);
    let steps = p.plan(&[bound(None, None)], &vs, t0, 0);
    let (_, text) = one(&steps);
    assert!(text.starts_with("*fix-bug*: starting (<1m)"), "{text}");
}

/// A view folded from `events`, each `(kind, seconds after the epoch)`.
fn folded_at(events: &[(&str, i64)]) -> View {
    let mut v = View::default();
    for (kind, secs) in events {
        v.fold(kind, &json!({}), secs * 1000);
    }
    v
}

#[test]
fn each_phase_is_named_with_the_time_spent_in_it() {
    let at = |events: &[(&str, i64)], now| render("b", &folded_at(events), now);
    assert_eq!(at(&[("started", 100)], 130), "*b*: starting (<1m)");
    assert_eq!(
        at(&[("setting_up", 100)], 100 + 300),
        "*b*: setting up (5m)"
    );
    let long = 160 + 3 * 3600 + 5 * 60;
    let working = [("setting_up", 100), ("working", 160), ("agent_said", long)];
    assert_eq!(at(&working, long), "*b*: working (3h 05m)");
    assert_eq!(
        at(&[("needs_input", 100)], 100 + 120),
        "*b*: waiting on a question (2m)"
    );
    assert_eq!(at(&[("asleep", 100)], 100 + 60), "*b*: asleep (1m)");
}

#[test]
fn a_repeated_event_of_the_same_phase_keeps_the_time_it_began() {
    let v = folded_at(&[("working", 100), ("busy", 400)]);
    assert_eq!(render("b", &v, 700), "*b*: working (10m)");
}

#[test]
fn a_working_box_silent_for_ten_minutes_gets_a_stall_hint() {
    let v = folded_at(&[("working", 0), ("agent_said", 100)]);
    let hint = "\nNo event for 10m. It may be stuck.";
    assert_eq!(render("b", &v, 100 + 599), "*b*: working (11m)");
    assert_eq!(
        render("b", &v, 100 + 600),
        format!("*b*: working (11m){hint}")
    );
    let idle = folded_at(&[("working", 0), ("idled", 100)]);
    assert!(!render("b", &idle, 100 + 3600).contains("stuck"));
}

#[test]
fn the_text_holds_still_within_a_minute_so_the_edit_loop_sends_nothing() {
    let t0 = crate::feed::now();
    let view = folded_at(&[("working", 0)]);
    let shown = render("fix-bug", &view, 61);
    let vs = views(view);
    let rows = [bound(Some("9.9"), Some(&shown))];
    let p = Planner::default();
    assert!(p.plan(&rows, &vs, t0, 119).is_empty());
    let steps = p.plan(&rows, &vs, t0, 120);
    assert_eq!(one(&steps).1, "*fix-bug*: working (2m)");
}

#[test]
fn a_closing_box_says_so_whatever_its_last_event() {
    let t0 = crate::feed::now();
    let vs = views(folded_at(&[("working", 0)]));
    let row = fed(
        RowState::Closing {
            box_id: ID,
            branch: None,
            discard: false,
        },
        Some("9.9"),
        Some("*fix-bug*: working (<1m)"),
    );
    let steps = Planner::default().plan(&[row], &vs, t0, 5);
    assert_eq!(one(&steps), (Some("9.9"), "*fix-bug*: closing"));
}

/// The blocks of the only step planned for the bound Box in `view`.
fn blocks_of(view: View, row: &FedBox) -> Value {
    let steps = Planner::default().plan(
        std::slice::from_ref(row),
        &views(view),
        crate::feed::now(),
        0,
    );
    assert_eq!(steps.len(), 1, "{steps:?}");
    steps.first().unwrap().blocks.clone()
}

/// The `(action_id, value)` of each button in `blocks`.
fn buttons(blocks: &Value) -> Vec<(String, String)> {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "actions")
        .flat_map(|b| b["elements"].as_array().cloned().unwrap_or_default())
        .map(|e| {
            let text = |key: &str| e[key].as_str().unwrap_or_default().to_owned();
            (text("action_id"), text("value"))
        })
        .collect()
}

#[test]
fn a_working_or_waiting_box_shows_a_stop_button_that_names_it() {
    let stop = vec![("stop_box".to_owned(), "fix-bug".to_owned())];
    let working = folded(&[("working", json!({}))]);
    let blocks = blocks_of(working, &bound(None, None));
    assert_eq!(buttons(&blocks), stop);
    let shown = blocks[0]["text"]["text"].as_str().unwrap();
    assert!(shown.starts_with("*fix-bug*: working"), "{shown}");
    let asking = folded(&[("needs_input", json!({"question": "Which db?"}))]);
    assert_eq!(buttons(&blocks_of(asking, &bound(None, None))), stop);
}

#[test]
fn any_other_state_sends_no_blocks_so_the_button_goes_away() {
    for events in [
        vec![],
        vec![("setting_up", json!({}))],
        vec![("working", json!({})), ("idled", json!({}))],
        vec![("asleep", json!({}))],
        vec![("rejected", json!({"reason": "no"}))],
        vec![("setup_failed", json!({"line": "x"}))],
        vec![("working", json!({})), ("closed", json!({}))],
    ] {
        let blocks = blocks_of(folded(&events), &bound(Some("9.9"), Some("old")));
        assert_eq!(blocks, json!([]), "{events:?}");
    }
}

#[test]
fn a_closing_or_gone_box_loses_the_button_even_while_its_view_says_working() {
    let working = folded(&[("working", json!({}))]);
    let row = |state| fed(state, Some("9.9"), Some("*fix-bug*: working"));
    let closing = RowState::Closing {
        box_id: ID,
        branch: None,
        discard: false,
    };
    let gone = RowState::Gone {
        box_id: ID,
        branch: None,
    };
    assert_eq!(blocks_of(working.clone(), &row(closing)), json!([]));
    assert_eq!(blocks_of(working, &row(gone)), json!([]));
}

#[test]
fn a_box_starting_for_three_minutes_gets_a_stuck_line_that_says_how_to_recover() {
    let v = folded_at(&[("started", 0), ("setting_up", 10), ("working", 20)]);
    assert!(!render("b", &v, 20 + 179).contains("stuck"));
    assert!(render("b", &v, 20 + 180).ends_with(
        "\nNo agent session after 3m. It may be stuck starting; ask me to restart the agent."
    ));
    let setting_up = folded_at(&[("started", 0), ("setting_up", 10)]);
    assert!(!render("b", &setting_up, 10 + 3600).contains("stuck"));
    let working = folded_at(&[
        ("started", 0),
        ("setting_up", 10),
        ("working", 20),
        ("session_started", 20),
    ]);
    assert!(!render("b", &working, 20 + 600 - 1).contains("No agent session"));
}

#[test]
fn a_box_whose_launch_failed_gets_the_stuck_line_once_working_for_three_minutes() {
    let v = folded_at(&[("working", 0), ("session_failed", 5)]);
    assert!(!render("b", &v, 179).contains("stuck"));
    assert!(render("b", &v, 180).ends_with(
        "\nNo agent session after 3m. It may be stuck starting; ask me to restart the agent."
    ));
}

#[test]
fn a_launch_that_timed_out_keeps_the_stuck_line_after_the_box_goes_quiet() {
    let mut v = folded_at(&[("working", 0), ("idled", 200)]);
    v.fold("launch_timed_out", &json!({"after_secs": 180}), 200_000);
    assert!(render("b", &v, 200).contains("No agent session after 3m."));
    let asleep = folded_at(&[("working", 0), ("asleep", 300)]);
    assert!(!render("b", &asleep, 3600).contains("stuck"));
}

#[test]
fn a_box_at_a_first_run_screen_for_three_minutes_names_the_screen() {
    let mut v = folded_at(&[("working", 0)]);
    v.fold(
        "agent_prompt_blocked",
        &json!({"prompt": "folder_trust"}),
        10_000,
    );
    assert!(!render("b", &v, 10 + 179).contains("Stopped at"));
    assert!(render("b", &v, 10 + 180).ends_with(
        "\nStopped at its folder_trust screen for 3m. It may be stuck; ask me to restart the agent."
    ));
}

#[test]
fn an_unanswered_question_after_three_minutes_says_to_answer_or_restart() {
    let v = folded_at(&[("working", 0), ("needs_input", 10)]);
    assert!(!render("b", &v, 10 + 179).contains("stuck"));
    assert!(render("b", &v, 10 + 180).ends_with(
        "\nWaiting on an answer for 3m. It may be stuck; reply here with the answer, or ask me to restart the agent."
    ));
}
