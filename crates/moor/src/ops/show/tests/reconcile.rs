//! Reconcile corrects a Box its host lists as stopped.

use super::*;
use crate::cx::owners::Cmd;
use moor_core::domain::hands::HandsInc;

#[test]
fn reconcile_queues_a_working_box_its_host_stopped_to_resume_first() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    let handle = cx
        .store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .hands
        .unwrap()
        .handle
        .unwrap();
    driver.stop_behind(&handle);
    assert_eq!(box_state(&cx), HandsState::Asleep);
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Pending);
    assert_eq!(row.session, SessionState::Working);
    assert_eq!(row.hands.unwrap().handle, Some(handle));
    let store = cx.store().unwrap();
    assert!(store.resume_first(id).unwrap().is_some());
    assert!(store.newest_event_of(id, "idled").unwrap().is_none());
    let said = store.newest_event_of(id, "asleep").unwrap().unwrap();
    assert_eq!(
        said.payload.get("reason").and_then(|r| r.as_str()),
        Some(moor_core::decide::reconcile::STOPPED_REASON)
    );
}

#[test]
fn reconcile_only_sleeps_a_quiet_box_its_host_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    let store = cx.store().unwrap();
    store
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    driver.stop_behind(&row.hands.unwrap().handle.unwrap());
    assert_eq!(box_state(&cx), HandsState::Asleep);
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Asleep);
    assert!(store.resume_first(id).unwrap().is_none());
}

#[test]
fn reconcile_leaves_a_box_alone_while_an_op_holds_its_lock() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    driver.stop_behind(&row.hands.unwrap().handle.unwrap());
    let held = cx.owners_arc().hold(id, Cmd::Sleep);
    assert_eq!(
        box_state(&cx),
        HandsState::Ready,
        "a machine that shows paused mid-suspend is that op's to record"
    );
    assert!(cx
        .store()
        .unwrap()
        .newest_event_of(id, "asleep")
        .unwrap()
        .is_none());
    drop(held);
    assert_eq!(box_state(&cx), HandsState::Asleep);
}

#[test]
fn reconcile_leaves_a_running_box_its_host_lists() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    assert_eq!(box_state(&cx), HandsState::Ready);
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
}

#[test]
fn reconcile_marks_gone_a_box_whose_host_answers_without_its_lease() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    let handle = cx
        .store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .hands
        .unwrap()
        .handle
        .unwrap();
    driver.forget(&handle);
    assert_eq!(box_state(&cx), HandsState::Gone);
}

#[test]
fn status_reads_of_an_asleep_box_make_no_guest_call_and_keep_its_session() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new());
    let engine = Arc::new(FakeEngine::new());
    let cx = cx(dir.path(), &driver).with_engine(engine.clone());
    let id = working(&cx, &driver, "a", Some("sess"));
    let store = cx.store().unwrap();
    let suspend = BoxLifecycleInput::Suspend {
        reason: "last-resort suspend".to_owned(),
    };
    store
        .apply(
            Decision::new(id)
                .lifecycle(suspend)
                .hands_input(HandsInc::FIRST, HandsInput::Sleep),
        )
        .unwrap();
    let before = driver.calls().len();
    run(&cx, &req(None, None)).unwrap();
    let shown = run(&cx, &req(Some("a"), None)).unwrap();
    let inspect = moor_core::request::InspectReq {
        target: BoxName::parse_allow_bare("a").unwrap().into(),
        said: None,
        lines: None,
    };
    crate::ops::inspect::run(&cx, &inspect).unwrap();
    assert_eq!(engine.probe_count("sess"), 0);
    assert!(engine.asked().is_empty());
    let guest: Vec<String> = driver
        .calls()
        .split_off(before)
        .into_iter()
        .filter(|c| !c.starts_with("list"))
        .collect();
    assert_eq!(guest, Vec::<String>::new());
    assert!(store.live_launch(id).unwrap().is_some());
    let agent = shown.boxed.unwrap().agent.unwrap();
    assert_eq!(agent.pane, "gone");
    assert_ne!(agent.state, "exited");
}

#[test]
fn reconcile_drops_a_verdict_on_hands_replaced_since_its_listing_and_warns() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    let store = cx.store().unwrap();
    let listed = store.box_row(id).unwrap().unwrap();
    driver.stop_behind(listed.hands.as_ref().unwrap().handle.as_ref().unwrap());
    store.replace_hands(id);
    drop(cx.take_warnings());

    crate::ops::common::reconcile::one(&cx, store, &listed);

    let codes: Vec<_> = cx.take_warnings().into_iter().map(|w| w.code).collect();
    assert_eq!(codes, ["stale_hands_write"]);
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(row.hands.unwrap().state, HandsState::Starting);
}

#[test]
fn a_dropped_stale_write_is_logged_by_the_controller_with_the_box_and_hands() {
    use crate::ops::common::stale::ControllerLog;
    use std::io::Write;
    use std::sync::Mutex;

    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let log = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink: Arc<Mutex<dyn Write + Send>> = log.clone();
    let cx = cx(dir.path(), &driver).sharing(ControllerLog::new(&sink));
    let id = working(&cx, &driver, "a", None);
    let store = cx.store().unwrap();
    let listed = store.box_row(id).unwrap().unwrap();
    driver.stop_behind(listed.hands.as_ref().unwrap().handle.as_ref().unwrap());
    store.replace_hands(id);

    crate::ops::common::reconcile::one(&cx, store, &listed);

    let line = String::from_utf8(log.lock().unwrap().clone()).unwrap();
    assert_eq!(
        line,
        format!(
            "moor controller: stale_hands_write: stale write dropped: Box {} has moved on from hands {}\n",
            id.get(),
            listed.hands.unwrap().inc.get()
        )
    );
}
