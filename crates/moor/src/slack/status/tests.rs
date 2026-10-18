use super::*;
use moor_core::BoxName;
use serde_json::json;

use crate::adapters::clock::SystemClock;

fn addr() -> SocketAddr {
    "127.0.0.1:8080".parse().unwrap()
}

fn thread(ts: Option<&str>) -> SlackThread {
    SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        task: BoxName::parse_allow_bare("fix-bug").unwrap(),
        owner: "U1".into(),
        status_ts: ts.map(str::to_owned),
    }
}

fn views(view: View) -> HashMap<String, View> {
    HashMap::from([("fix-bug".to_owned(), view)])
}

fn folded(events: &[(&str, Value)]) -> View {
    let mut v = View::default();
    for (kind, payload) in events {
        v.fold(kind, payload);
    }
    v
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
    assert!(render("fix-bug", &v, None, None).contains("needs input: Which db?"));
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
    let text = render("fix-bug", &v, None, None);
    assert!(text.contains("closed"), "{text}");
    assert!(text.contains("Box: asleep"), "{text}");
    assert_eq!(
        folded(&[("box_gone", json!({}))]).boxed,
        Some(BoxState::Gone)
    );
}

#[test]
fn question_cannot_ping() {
    let v = folded(&[("needs_input", json!({"question": "<!channel> hi"}))]);
    assert!(!render("t", &v, None, None).contains("<!channel>"));
}

#[test]
fn web_link_only_when_known() {
    let v = View::default();
    let with = render("fix-bug", &v, Some(addr()), None);
    assert!(with.contains("http://127.0.0.1:8080/"));
    assert!(with.contains("ssh -L"));
    assert!(!render("fix-bug", &v, None, None).contains("http"));
}

#[test]
fn posts_once_then_updates() {
    let t0 = SystemClock::real().instant();
    let mut p = Planner::default();
    let mut vs = views(View::default());
    let steps = p.plan(&[thread(None)], &vs, None, &HashMap::new(), t0);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps.first().unwrap().ts, None);
    p.record(steps.first().unwrap(), t0, true);

    let later = t0 + UPDATE_EVERY;
    assert!(p
        .plan(&[thread(Some("9.9"))], &vs, None, &HashMap::new(), later)
        .is_empty());

    vs.get_mut("fix-bug").unwrap().fold("working", &json!({}));
    let steps = p.plan(&[thread(Some("9.9"))], &vs, None, &HashMap::new(), later);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps.first().unwrap().ts.as_deref(), Some("9.9"));
    assert!(steps.first().unwrap().text.contains("working"));
}

#[test]
fn quick_changes_coalesce_into_one_later_update() {
    let t0 = SystemClock::real().instant();
    let mut p = Planner::default();
    let mut vs = views(View::default());
    let th = [thread(Some("9.9"))];
    let first = p.plan(&th, &vs, None, &HashMap::new(), t0);
    p.record(first.first().unwrap(), t0, true);

    vs.get_mut("fix-bug").unwrap().fold("working", &json!({}));
    let soon = t0 + Duration::from_millis(500);
    assert!(p.plan(&th, &vs, None, &HashMap::new(), soon).is_empty());
    vs.get_mut("fix-bug").unwrap().fold("idled", &json!({}));
    assert!(p.plan(&th, &vs, None, &HashMap::new(), soon).is_empty());

    let due = t0 + UPDATE_EVERY;
    let steps = p.plan(&th, &vs, None, &HashMap::new(), due);
    assert_eq!(steps.len(), 1);
    assert!(steps.first().unwrap().text.contains("idle"));
    p.record(steps.first().unwrap(), due, true);
    assert!(p
        .plan(&th, &vs, None, &HashMap::new(), due + UPDATE_EVERY)
        .is_empty());
}

#[test]
fn failed_send_stays_due() {
    let t0 = SystemClock::real().instant();
    let mut p = Planner::default();
    let vs = views(View::default());
    let th = [thread(None)];
    let steps = p.plan(&th, &vs, None, &HashMap::new(), t0);
    p.record(steps.first().unwrap(), t0, false);
    assert!(p
        .plan(
            &th,
            &vs,
            None,
            &HashMap::new(),
            t0 + Duration::from_millis(1)
        )
        .is_empty());
    assert_eq!(
        p.plan(&th, &vs, None, &HashMap::new(), t0 + UPDATE_EVERY)
            .len(),
        1
    );
}

#[test]
fn pr_link_only_when_known() {
    let v = View::default();
    let with = render("fix-bug", &v, None, Some("https://github.com/o/r/pull/7"));
    assert!(
        with.contains("<https://github.com/o/r/pull/7|Pull request>"),
        "{with}"
    );
    assert!(!render("fix-bug", &v, None, None).contains("Pull request"));
}

#[test]
fn appearing_pr_link_triggers_an_edit() {
    let t0 = SystemClock::real().instant();
    let mut p = Planner::default();
    let vs = views(View::default());
    let th = [thread(Some("9.9"))];
    let first = p.plan(&th, &vs, None, &HashMap::new(), t0);
    p.record(first.first().unwrap(), t0, true);
    let due = t0 + UPDATE_EVERY;
    assert!(p.plan(&th, &vs, None, &HashMap::new(), due).is_empty());

    let prs = HashMap::from([("fix-bug".to_owned(), "https://x/pull/1".to_owned())]);
    let steps = p.plan(&th, &vs, None, &prs, due);
    assert_eq!(steps.len(), 1);
    assert!(steps.first().unwrap().text.contains("https://x/pull/1"));
}
