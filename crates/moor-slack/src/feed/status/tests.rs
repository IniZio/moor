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
        v.fold(kind, payload);
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
    assert!(render("fix-bug", &v, "", None).contains("needs input: Which db?"));
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
    let text = render("fix-bug", &v, "", None);
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
    assert!(!render("t", &v, "", None).contains("<!channel>"));
}

#[test]
fn the_status_links_pull_requests_from_its_branch_only_for_an_owner_and_name_repo() {
    let v = View::default();
    let text = render("fix-bug", &v, "acme/app", Some("moor/fix bug"));
    assert!(
        text.ends_with(
            "\nPull request: <https://github.com/acme/app/pulls?q=is%3Apr+head%3Amoor/fix%20bug|moor/fix bug>"
        ),
        "{text}"
    );
    for repo in ["demo", "acme/app/x", "acme/<!here>", "/app"] {
        let text = render("fix-bug", &v, repo, Some("moor/fix-bug"));
        assert!(!text.contains("Pull request"), "{repo}: {text}");
    }
    assert!(!render("fix-bug", &v, "acme/app", None).contains("Pull request"));
}

#[test]
fn posts_once_then_updates() {
    let t0 = crate::feed::now();
    let mut p = Planner::default();
    let mut vs = views(View::default());
    let steps = p.plan(&[bound(None, None)], &vs, t0);
    let (ts, text) = one(&steps);
    assert_eq!(ts, None);
    let first = text.to_owned();
    p.record(steps.first().unwrap(), t0);

    let later = t0 + UPDATE_EVERY;
    assert!(p
        .plan(&[bound(Some("9.9"), Some(&first))], &vs, later)
        .is_empty());

    vs.get_mut(&ID).unwrap().fold("working", &json!({}));
    let steps = p.plan(&[bound(Some("9.9"), Some(&first))], &vs, later);
    let (ts, text) = one(&steps);
    assert_eq!(ts, Some("9.9"));
    assert!(text.contains("working"), "{text}");
}

#[test]
fn quick_changes_coalesce_into_one_later_update() {
    let t0 = crate::feed::now();
    let mut p = Planner::default();
    let mut vs = views(View::default());
    let first = p.plan(&[bound(Some("9.9"), None)], &vs, t0);
    p.record(first.first().unwrap(), t0);
    let shown = first.first().unwrap().text.clone();
    let rows = [bound(Some("9.9"), Some(&shown))];

    vs.get_mut(&ID).unwrap().fold("working", &json!({}));
    let soon = t0 + Duration::from_millis(500);
    assert!(p.plan(&rows, &vs, soon).is_empty());
    vs.get_mut(&ID).unwrap().fold("idled", &json!({}));
    assert!(p.plan(&rows, &vs, soon).is_empty());

    let due = t0 + UPDATE_EVERY;
    let steps = p.plan(&rows, &vs, due);
    let (_, text) = one(&steps);
    assert!(text.contains("idle"), "{text}");
    let shown = text.to_owned();
    p.record(steps.first().unwrap(), due);
    let rows = [bound(Some("9.9"), Some(&shown))];
    assert!(p.plan(&rows, &vs, due + UPDATE_EVERY).is_empty());
}

#[test]
fn failed_send_stays_due() {
    let t0 = crate::feed::now();
    let mut p = Planner::default();
    let vs = views(View::default());
    let rows = [bound(None, None)];
    let steps = p.plan(&rows, &vs, t0);
    p.record(steps.first().unwrap(), t0);
    assert!(p.plan(&rows, &vs, t0 + Duration::from_millis(1)).is_empty());
    assert_eq!(p.plan(&rows, &vs, t0 + UPDATE_EVERY).len(), 1);
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
    assert!(p.plan(&[gone(None)], &vs, t0).is_empty(), "no new message");
    let steps = p.plan(&[gone(Some("9.9"))], &vs, t0);
    assert_eq!(one(&steps), (Some("9.9"), "*fix-bug*: closed"));
}

#[test]
fn each_box_follows_only_the_events_of_its_own_id() {
    let t0 = crate::feed::now();
    let p = Planner::default();
    let vs = HashMap::from([(BoxId::new(8), folded(&[("working", json!({}))]))]);
    let steps = p.plan(&[bound(None, None)], &vs, t0);
    let (_, text) = one(&steps);
    assert!(text.starts_with("*fix-bug*: starting"), "{text}");
}
