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
        },
        Some("9.9"),
        Some("*fix-bug*: working (<1m)"),
    );
    let steps = Planner::default().plan(&[row], &vs, t0, 5);
    assert_eq!(one(&steps), (Some("9.9"), "*fix-bug*: closing"));
}
