use super::*;

/// A Box `name` whose primary Session is a child of Session `parent`.
fn add_child(store: &Store, name: &str, parent: i64) -> BoxId {
    let new = NewBox {
        name: BoxName::parse_allow_bare(name).expect("name"),
        repo: None,
    };
    let decision = Decision::insert(new).parent_session(Some(parent));
    store.apply(decision).expect("insert child").box_id
}

fn only_parent(parent: i64) -> WatchFilter {
    WatchFilter {
        parent: Some(SessionId::new(parent)),
        ..WatchFilter::default()
    }
}

/// The Boxes named by the event frames in `got`, without the caught-up frame.
fn event_boxes(got: &[Value]) -> Vec<String> {
    got.iter()
        .filter(|f| f["frame"] == "event")
        .map(|f| f["box"].as_str().expect("box").to_owned())
        .collect()
}

/// Two parents with one child Box each, and an unrelated Box with no parent.
fn family(dir: &Path) -> (i64, i64) {
    let store = open(dir);
    let one = add(&store, "one");
    let two = add(&store, "two");
    let session = |id: BoxId| {
        store
            .events_after(0, 10_000)
            .expect("events")
            .iter()
            .find(|e| e.row.box_id == id && e.session.is_some())
            .and_then(|e| e.session)
            .expect("a session event")
            .get()
    };
    let (p1, p2) = (session(one), session(two));
    let c1 = add_child(&store, "kid-one", p1);
    let c2 = add_child(&store, "kid-two", p2);
    append(&store, c1, 1);
    append(&store, c2, 1);
    (p1, p2)
}

#[test]
fn parent_filter_delivers_only_the_children_of_that_parent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (p1, _) = family(dir.path());
    let got = read_once(&dir, 0, &only_parent(p1));
    let boxes = event_boxes(&got);
    assert!(!boxes.is_empty(), "{got:?}");
    assert!(boxes.iter().all(|b| b == "kid-one"), "{boxes:?}");
}

#[test]
fn parent_filter_with_no_children_delivers_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    family(dir.path());
    let got = read_once(&dir, 0, &only_parent(9_999));
    assert!(event_boxes(&got).is_empty(), "{got:?}");
    assert_eq!(got.last().expect("frame")["frame"], "caught-up");
}

#[test]
fn parent_filter_keeps_the_viewers_own_boxes_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    let raw = rusqlite::Connection::open(dir.path().join(STORE_FILE)).expect("raw");
    raw.execute(
        "INSERT INTO users (id, created_at) VALUES (1, 0), (2, 0)",
        [],
    )
    .expect("users");
    add(&store, "root");
    let parent = store
        .events_after(0, 10)
        .expect("events")
        .iter()
        .find_map(|e| e.session)
        .expect("session")
        .get();
    for (name, user) in [("mine", 1), ("theirs", 2)] {
        let new = NewBox {
            name: BoxName::parse_allow_bare(name).expect("name"),
            repo: None,
        };
        let decision = Decision::insert(new)
            .parent_session(Some(parent))
            .requester(UserId::new(user));
        let id = store.apply(decision).expect("insert").box_id;
        append(&store, id, 1);
    }
    let (mut probe, out) = Probe::new(usize::MAX);
    let viewer = Viewer::User(UserId::new(1));
    let mut asked = ask(0, only_parent(parent), false);
    asked.viewer = viewer;
    stream(&mut probe, &env(&dir), asked, fast(64));
    let boxes = event_boxes(&frames(&out));
    assert!(!boxes.is_empty());
    assert!(boxes.iter().all(|b| b == "mine"), "{boxes:?}");
}

#[test]
fn an_event_with_no_session_never_matches_a_parent_filter() {
    let dir = tempfile::tempdir().expect("tempdir");
    let moor = crate::Moor::from_env(&env(&dir)).expect("moor");
    let asked = ask(0, only_parent(1), false);
    let mut passes = crate::controller::watch::Passes {
        moor: &moor,
        ask: &asked,
        seen: std::collections::HashMap::new(),
        parents: std::collections::HashMap::new(),
    };
    let frame = moor_core::controller::ServerFrame::Event {
        cursor: 1,
        r#box: None,
        box_id: None,
        session: None,
        kind: "copied".to_owned(),
        payload: json!({}),
        ts: 1,
    };
    assert_eq!(passes.check(&frame), Ok(false));
}
