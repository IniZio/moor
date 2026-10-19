use super::{
    child, close, end_request, end_session, ended, ended_at, front, mutations, Script, ROOT,
};

#[test]
fn a_worker_with_an_ended_session_still_has_its_box_closed() {
    let script = Script {
        tree: vec![ended(child(ROOT, 10, Some(("a", 1))))],
        boxes: vec![(1, "a", "idle")],
        ..Script::default()
    };
    let f = front(&script, None);
    let job = end_request(&f);
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(mutations(&f), vec![close("a"), end_session(ROOT)]);
}

#[test]
fn an_ended_child_whose_box_is_already_closed_is_skipped() {
    let script = Script {
        tree: vec![ended(child(ROOT, 10, Some(("a", 1))))],
        boxes: vec![(1, "a", "closed")],
        ..Script::default()
    };
    let f = front(&script, None);
    let job = end_request(&f);
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(mutations(&f), vec![end_session(ROOT)]);
}

#[test]
fn an_ended_host_child_gets_no_end_host_session() {
    let script = Script {
        tree: vec![child(ROOT, 10, None), ended(child(ROOT, 11, None))],
        ..Script::default()
    };
    let f = front(&script, None);
    let job = end_request(&f);
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(mutations(&f), vec![end_session(10), end_session(ROOT)]);
}

#[test]
fn a_reused_box_name_that_belongs_to_another_request_is_not_closed() {
    let script = Script {
        tree: vec![ended(child(ROOT, 10, Some(("a", 1))))],
        boxes: vec![(1, "a", "closed"), (9, "a", "working")],
        ..Script::default()
    };
    let f = front(&script, None);
    let job = end_request(&f);
    assert_eq!(job["state"], "done", "{job}");
    assert_eq!(mutations(&f), vec![end_session(ROOT)]);
}

#[test]
fn a_box_without_an_id_fails_the_close_and_keeps_the_root() {
    let script = Script {
        tree: vec![ended(child(ROOT, 10, Some(("a", 0))))],
        boxes: vec![(1, "a", "idle")],
        ..Script::default()
    };
    let f = front(&script, None);
    let job = end_request(&f);
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(job["failure"]["step"], "close-children");
    assert!(job["failure"]["message"]
        .as_str()
        .unwrap()
        .contains("no id for Box a"));
    assert_eq!(mutations(&f), vec![]);
    assert_eq!(ended_at(&f), None);
}

#[test]
fn a_root_that_already_ended_still_finishes_the_request() {
    let script = Script {
        root_ended: true,
        ..Script::default()
    };
    let f = front(&script, None);
    let job = end_request(&f);
    assert_eq!(job["state"], "done", "{job}");
    assert!(ended_at(&f).is_some());
}
