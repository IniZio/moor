//! Reconcile corrects a Box its host lists as stopped.

use super::*;

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
        .boxed
        .unwrap()
        .handle
        .unwrap();
    driver.stop_behind(&handle);
    assert_eq!(box_state(&cx), BoxState::Asleep);
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Pending);
    assert_eq!(row.session, SessionState::Working);
    assert_eq!(row.boxed.unwrap().handle, Some(handle));
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
    driver.stop_behind(&row.boxed.unwrap().handle.unwrap());
    assert_eq!(box_state(&cx), BoxState::Asleep);
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
    driver.stop_behind(&row.boxed.unwrap().handle.unwrap());
    let held = crate::ops::common::boxlock::BoxLock::acquire(
        cx.clock(),
        cx.state_dir().unwrap(),
        id,
        "sleep",
        std::time::Duration::ZERO,
    )
    .unwrap();
    assert_eq!(
        box_state(&cx),
        BoxState::Ready,
        "a machine that shows paused mid-suspend is that op's to record"
    );
    assert!(cx
        .store()
        .unwrap()
        .newest_event_of(id, "asleep")
        .unwrap()
        .is_none());
    drop(held);
    assert_eq!(box_state(&cx), BoxState::Asleep);
}

#[test]
fn reconcile_leaves_a_running_box_its_host_lists() {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(FakeDriver::new().listed_by_handle());
    let cx = cx(dir.path(), &driver);
    let id = working(&cx, &driver, "a", None);
    assert_eq!(box_state(&cx), BoxState::Ready);
    let row = cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
}

#[test]
fn reconcile_keeps_a_box_its_host_no_longer_lists() {
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
        .boxed
        .unwrap()
        .handle
        .unwrap();
    driver.forget(&handle);
    assert_eq!(box_state(&cx), BoxState::Ready);
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
                .box_input(BoxInput::Sleep),
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
    assert!(store.live_session(id).unwrap().is_some());
    let agent = shown.boxed.unwrap().agent.unwrap();
    assert_eq!(agent.pane, "gone");
    assert_ne!(agent.state, "exited");
}
