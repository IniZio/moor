use std::sync::Arc;

use moor_core::activity::{Activity, AgentState, Source};
use moor_core::config::{parse_user, UserConfig};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId, RunId};

use super::{agent_view, box_view, box_view_with_run, event_view, rfc3339};
use crate::ops::common::activity::{PaneLiveness, Reading};
use crate::store::{RunRow, Store};
use crate::testing::clock::FakeClock;

#[test]
fn rfc3339_formats_epoch_and_leap_day() {
    assert_eq!(rfc3339(Ts::from_millis(0)), "1970-01-01T00:00:00Z");
    assert_eq!(
        rfc3339(Ts::from_millis(1_709_210_096_999)),
        "2024-02-29T12:34:56Z"
    );
    assert_eq!(rfc3339(Ts::from_millis(-1000)), "1969-12-31T23:59:59Z");
}

#[test]
fn rfc3339_out_of_range_falls_back_to_raw_millis() {
    assert_eq!(rfc3339(Ts::from_millis(i64::MAX)), i64::MAX.to_string());
    assert_eq!(rfc3339(Ts::from_millis(i64::MIN)), i64::MIN.to_string());
}

#[test]
fn box_view_maps_row_without_box() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(86_400_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("t").unwrap(),
        repo: Some(crate::testing::cx::box_repo(None)),
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let fields = Fields {
        prompt: Field::Set("g".into()),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(fields)).unwrap();
    let v = box_view(&store.box_row(id).unwrap().unwrap(), &UserConfig::default());
    assert_eq!(
        (
            v.id,
            v.name.as_str(),
            v.repo.as_ref().map(ToString::to_string)
        ),
        (id.get(), "t", Some("o/n".to_owned()))
    );
    assert_eq!(v.prompt.as_deref(), Some("g"));
    assert!(v.driver.is_none() && v.agent.is_none() && v.run.is_none());
    assert_eq!(v.updated_at, "1970-01-02T00:00:00Z");
    let events = store.events(id, 10).unwrap();
    let ev = event_view(events.first().unwrap());
    assert_eq!(ev.id, events.first().unwrap().id);
    assert_eq!(ev.ts, "1970-01-02T00:00:00Z");
}

#[test]
fn box_view_maps_box() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("t").unwrap(),
        repo: None,
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        ..Fields::default()
    };
    store
        .apply(Decision::new(id).set(fields).box_input(BoxInput::Requested))
        .unwrap();
    store
        .apply(Decision::new(id).box_input(BoxInput::Acquired {
            handle: BoxHandle::new("h"),
        }))
        .unwrap();
    let b = box_view(&store.box_row(id).unwrap().unwrap(), &UserConfig::default())
        .driver
        .unwrap();
    assert_eq!(
        (b.state, b.provider.as_str(), b.handle.as_deref()),
        (BoxState::Ready, "p", Some("h"))
    );
    assert!(b.asleep_since.is_none());
}

#[test]
fn box_view_maps_sleep_reason() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("t").unwrap(),
        repo: None,
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        ..Fields::default()
    };
    store
        .apply(Decision::new(id).set(fields).box_input(BoxInput::Requested))
        .unwrap();
    let b = box_view(&store.box_row(id).unwrap().unwrap(), &UserConfig::default())
        .driver
        .unwrap();
    assert_eq!(
        (b.sleep.after.as_deref(), b.sleep.reason),
        (Some("30m"), None)
    );
    let fields = Fields {
        box_sleep_reason: Field::Set("provider cannot wake".into()),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(fields)).unwrap();
    let b = box_view(&store.box_row(id).unwrap().unwrap(), &UserConfig::default())
        .driver
        .unwrap();
    assert_eq!(b.sleep.after.as_deref(), Some("30m"));
    assert_eq!(b.sleep.reason.as_deref(), Some("provider cannot wake"));
}

#[test]
fn box_view_shows_configured_sleep_after() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("t").unwrap(),
        repo: None,
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let fields = Fields {
        box_provider: Field::Set(ProviderId::new("p")),
        ..Fields::default()
    };
    store
        .apply(Decision::new(id).set(fields).box_input(BoxInput::Requested))
        .unwrap();
    let config: UserConfig = parse_user("u.yaml", "box: {sleep_after: 45m}").unwrap();
    let b = box_view(&store.box_row(id).unwrap().unwrap(), &config)
        .driver
        .unwrap();
    assert_eq!(b.sleep.after.as_deref(), Some("45m"));
}

#[test]
fn box_view_with_run_fills_run() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(0)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("t").unwrap(),
        repo: None,
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    let row = store.box_row(id).unwrap().unwrap();
    let run = RunRow {
        id: RunId::new(5),
        box_id: BoxId::new(id.get()),
        argv: vec!["make".into(), "test".into()],
        record: None,
        sha: None,
        pid: 1,
        exit: None,
        interrupted: false,
        started_at: Ts::from_millis(86_400_000),
        finished_at: None,
    };
    let r = box_view_with_run(&row, Some(&run), &UserConfig::default())
        .run
        .unwrap();
    assert_eq!(r.id, 5);
    assert_eq!(r.argv, ["make", "test"]);
    assert_eq!(r.started_at, "1970-01-02T00:00:00Z");
    assert!(box_view_with_run(&row, None, &UserConfig::default())
        .run
        .is_none());
}

fn reading(state: AgentState, source: Source, pane: PaneLiveness) -> Reading {
    Reading::new(Some(Activity { state, source }), pane, false, false)
}

#[test]
fn agent_view_is_none_without_activity() {
    let r = Reading::new(None, PaneLiveness::Gone, true, false);
    assert_eq!(agent_view(&r), None);
}

#[test]
fn agent_view_working_maps_state_source_and_pane() {
    let v = agent_view(&reading(
        AgentState::Working,
        Source::HooksCached,
        PaneLiveness::Live,
    ))
    .unwrap();
    assert_eq!(v.pane, "live");
    assert_eq!(v.state, "working");
    assert_eq!(v.source, "hooks_cached");
    assert_eq!(v.exit_code, None);
    assert_eq!(v.question, None);
    assert!(v.options.is_empty());
}

#[test]
fn agent_view_needs_input_carries_question_and_options() {
    let v = agent_view(&reading(
        AgentState::NeedsInput {
            question: Some("Proceed?".into()),
            options: vec!["Yes".into(), "No".into()],
        },
        Source::Pane,
        PaneLiveness::Live,
    ))
    .unwrap();
    assert_eq!(v.state, "needs_input");
    assert_eq!(v.source, "pane");
    assert_eq!(v.question.as_deref(), Some("Proceed?"));
    assert_eq!(v.options, ["Yes", "No"]);
    assert_eq!(v.exit_code, None);
}

#[test]
fn agent_view_exited_carries_exit_code_and_pane() {
    let v = agent_view(&reading(
        AgentState::Exited { code: 3 },
        Source::Herdr,
        PaneLiveness::Exited,
    ))
    .unwrap();
    assert_eq!(v.pane, "exited");
    assert_eq!(v.state, "exited");
    assert_eq!(v.source, "herdr");
    assert_eq!(v.exit_code, Some(3));
    assert_eq!(v.question, None);
    assert!(v.options.is_empty());
}

#[test]
fn agent_view_gone_pane_string() {
    let v = agent_view(&reading(
        AgentState::Unknown,
        Source::Timeout,
        PaneLiveness::Gone,
    ))
    .unwrap();
    assert_eq!(v.pane, "gone");
    assert_eq!(v.state, "unknown");
    assert_eq!(v.source, "timeout");
}
