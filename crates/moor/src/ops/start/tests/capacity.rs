use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use moor_core::config::{BoxConfig, DriverConfig, UserConfig};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::proc::Tool;
use moor_core::{BoxId, BoxName, ProviderId, StartReq};
use serde_json::json;

use super::state::WORKING;
use crate::cx::Cx;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::surface::FakeSurface;
use moor_core::domain::box_state::BoxLifecycle;

const PROVIDER: &str = "local-container";

struct World {
    cx: Cx,
    driver: Arc<FakeDriver>,
    surface: Arc<FakeSurface>,
    _dir: tempfile::TempDir,
}

fn world(config: UserConfig) -> World {
    let repo = Path::new("/repo");
    let dir = tempfile::tempdir().unwrap();
    let proc = Arc::new(ScriptedProc::new());
    proc.on(
        Tool::Git,
        &["rev-parse", "--show-toplevel"],
        Reply::Ok(format!("{}\n", repo.display())),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "HEAD"],
        Reply::Ok("abc123\n".into()),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Reply::Ok(format!("{}/.git\n", repo.display())),
    );
    proc.on(
        Tool::Git,
        &["rev-parse", "--git-path", "info/exclude"],
        Reply::Ok(format!("{}/exclude\n", dir.path().display())),
    );
    proc.on(Tool::Git, &["ls-tree"], Reply::Ok(String::new()));
    let driver = Arc::new(FakeDriver::new());
    let surface = Arc::new(FakeSurface::new(dir.path()));
    let cx = Cx::fake_with(
        dir.path(),
        repo,
        driver.clone(),
        surface.clone(),
        proc,
        config,
    );
    World {
        cx,
        driver,
        surface,
        _dir: dir,
    }
}

fn limit(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}

fn box_limit(n: u32) -> UserConfig {
    UserConfig {
        box_: BoxConfig {
            max_boxes: Some(limit(n)),
            ..BoxConfig::default()
        },
        ..UserConfig::default()
    }
}

fn driver_limit(provider: &str, n: u32) -> BTreeMap<String, DriverConfig> {
    BTreeMap::from([(
        provider.to_owned(),
        DriverConfig {
            max_boxes: Some(limit(n)),
            lookup_timeout_secs: None,
            strip_env: Vec::new(),
        },
    )])
}

fn req(name: &str) -> StartReq {
    StartReq {
        name: BoxName::parse_allow_bare(name).unwrap(),
        goal: None,
        issue: None,
        worktree: None,
        base: None,
        model: None,
        no_agent: false,
        hold: false,
        resume: false,
        trust_moor_yaml: false,
        repo: None,
        parent: None,
    }
}

fn lifecycle_of(w: &World, id: BoxId) -> BoxLifecycle {
    w.cx.store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .lifecycle
}

/// Frees the machine of Box `id` as if the driver lost it.
fn free_box(w: &World, id: BoxId) {
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
}

/// Moves the machine row of Box `id` to `provider`.
fn move_box_to(w: &World, id: BoxId, provider: &str) {
    let set = Fields {
        box_provider: Field::Set(ProviderId::new(provider)),
        ..Fields::default()
    };
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).set(set))
        .unwrap();
}

/// Starts `holder` under the limit, so one Box is live.
fn with_live_box(config: UserConfig) -> (World, BoxId) {
    let w = world(config);
    let v = run(&w.cx, &req("holder")).unwrap();
    assert!(!v.queued);
    (w, BoxId::new(v.boxed.id))
}

#[test]
fn start_queues_when_box_limit_reached() {
    let (w, _) = with_live_box(box_limit(1));
    let v = run(&w.cx, &req("waiter")).unwrap();
    assert!(v.queued);
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::WaitingForBox);
    assert_eq!(
        lifecycle_of(&w, BoxId::new(v.boxed.id)),
        BoxLifecycle::WaitingForBox
    );
}

#[test]
fn queued_start_records_no_session() {
    let (w, _) = with_live_box(box_limit(1));
    let v = run(&w.cx, &req("waiter")).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::WaitingForBox);
    let id = BoxId::new(v.boxed.id);
    let store = w.cx.store().unwrap();
    assert!(store.live_session(id).unwrap().is_none());
    assert!(!store.has_session(id).unwrap());
}

#[test]
fn queued_start_records_the_limit_and_keeps_worktree_and_workspace() {
    let (w, _) = with_live_box(box_limit(1));
    let v = run(&w.cx, &req("waiter")).unwrap();
    let events =
        w.cx.store()
            .unwrap()
            .events(BoxId::new(v.boxed.id), 50)
            .unwrap();
    let ev = events
        .iter()
        .find(|e| e.kind == "waiting_for_box")
        .expect("waiting_for_box event");
    assert_eq!(ev.payload.get("max_boxes"), Some(&json!(1)));
    let calls = w.surface.calls();
    assert!(calls.contains(&"create_worktree waiter".to_owned()));
    assert!(calls.contains(&"ensure_workspace moor:waiter".to_owned()));
    assert_eq!(w.driver.calls(), ["acquire", "exec box-1", "sweep box-1"]);
}

#[test]
fn queued_start_warns_with_the_box_config_key() {
    let (w, _) = with_live_box(box_limit(1));
    let v = run(&w.cx, &req("waiter")).unwrap();
    assert!(v
        .warnings_inline
        .iter()
        .any(|m| m.contains("box.max_boxes")));
}

#[test]
fn queued_start_warns_with_the_driver_config_key() {
    let config = UserConfig {
        drivers: driver_limit(PROVIDER, 1),
        ..UserConfig::default()
    };
    let (w, _) = with_live_box(config);
    let v = run(&w.cx, &req("waiter")).unwrap();
    assert!(v.queued);
    let key = format!("drivers.{PROVIDER}.max_boxes");
    assert!(v.warnings_inline.iter().any(|m| m.contains(&key)));
    let events =
        w.cx.store()
            .unwrap()
            .events(BoxId::new(v.boxed.id), 50)
            .unwrap();
    let ev = events.iter().find(|e| e.kind == "waiting_for_box").unwrap();
    assert_eq!(ev.payload.get("max_boxes"), Some(&json!(1)));
}

#[test]
fn start_again_while_still_full_keeps_waiting() {
    let (w, _) = with_live_box(box_limit(1));
    run(&w.cx, &req("waiter")).unwrap();
    let again = run(&w.cx, &req("waiter")).unwrap();
    assert!(again.queued);
    assert_eq!(again.boxed.lifecycle, BoxLifecycle::WaitingForBox);
    assert_eq!(w.driver.calls(), ["acquire", "exec box-1", "sweep box-1"]);
}

#[test]
fn start_again_after_a_box_is_freed_promotes_the_waiting_box() {
    let (w, holder) = with_live_box(box_limit(1));
    let queued = run(&w.cx, &req("waiter")).unwrap();
    assert!(queued.queued);
    assert_eq!(w.driver.calls(), ["acquire", "exec box-1", "sweep box-1"]);
    let id = BoxId::new(queued.boxed.id);
    free_box(&w, holder);
    let v = run(&w.cx, &req("waiter")).unwrap();
    assert!(!v.queued);
    assert_ne!(lifecycle_of(&w, id), BoxLifecycle::WaitingForBox);
    assert_eq!(
        w.driver.calls(),
        [
            "acquire",
            "exec box-1",
            "sweep box-1",
            "acquire",
            "exec box-2"
        ]
    );
}

#[test]
fn start_again_after_the_holder_closes_promotes_the_waiting_box() {
    let (w, holder) = with_live_box(box_limit(1));
    let queued = run(&w.cx, &req("waiter")).unwrap();
    assert!(queued.queued);
    assert_eq!(w.driver.calls(), ["acquire", "exec box-1", "sweep box-1"]);
    let id = BoxId::new(queued.boxed.id);
    let store = w.cx.store().unwrap();
    store
        .apply(
            Decision::new(holder)
                .lifecycle(BoxLifecycleInput::Close)
                .session(SessionInput::End)
                .box_input(BoxInput::Released),
        )
        .unwrap();
    let v = run(&w.cx, &req("waiter")).unwrap();
    assert!(!v.queued);
    assert_ne!(lifecycle_of(&w, id), BoxLifecycle::WaitingForBox);
    assert_eq!(
        w.driver.calls(),
        [
            "acquire",
            "exec box-1",
            "sweep box-1",
            "acquire",
            "exec box-2"
        ]
    );
}

#[test]
fn start_acquires_without_any_limit() {
    let (w, _) = with_live_box(UserConfig::default());
    let v = run(&w.cx, &req("second")).unwrap();
    assert!(!v.queued);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert_eq!(
        w.driver.calls(),
        [
            "acquire",
            "exec box-1",
            "sweep box-1",
            "acquire",
            "exec box-2",
            "sweep box-2"
        ]
    );
}

#[test]
fn start_acquires_below_the_limit() {
    let (w, _) = with_live_box(box_limit(2));
    let v = run(&w.cx, &req("second")).unwrap();
    assert!(!v.queued);
    assert_eq!(
        w.driver.calls(),
        [
            "acquire",
            "exec box-1",
            "sweep box-1",
            "acquire",
            "exec box-2",
            "sweep box-2"
        ]
    );
}

#[test]
fn limit_ignores_live_boxes_of_another_provider() {
    let (w, holder) = with_live_box(box_limit(1));
    move_box_to(&w, holder, "other-provider");
    let v = run(&w.cx, &req("second")).unwrap();
    assert!(!v.queued);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert_eq!(
        w.driver.calls(),
        [
            "acquire",
            "exec box-1",
            "sweep box-1",
            "acquire",
            "exec box-2",
            "sweep box-2"
        ]
    );
}

#[test]
fn driver_limit_ignores_live_boxes_of_another_provider() {
    let config = UserConfig {
        drivers: driver_limit(PROVIDER, 1),
        ..UserConfig::default()
    };
    let (w, holder) = with_live_box(config);
    move_box_to(&w, holder, "other-provider");
    let v = run(&w.cx, &req("second")).unwrap();
    assert!(!v.queued);
    assert_eq!(
        w.driver.calls(),
        [
            "acquire",
            "exec box-1",
            "sweep box-1",
            "acquire",
            "exec box-2",
            "sweep box-2"
        ]
    );
}

#[test]
fn driver_limit_overrides_the_box_limit() {
    let config = UserConfig {
        drivers: driver_limit(PROVIDER, 2),
        ..box_limit(1)
    };
    let (w, _) = with_live_box(config);
    let v = run(&w.cx, &req("second")).unwrap();
    assert!(!v.queued);
    assert_eq!((v.boxed.lifecycle, v.boxed.session_state), WORKING);
    assert_eq!(
        w.driver.calls(),
        [
            "acquire",
            "exec box-1",
            "sweep box-1",
            "acquire",
            "exec box-2",
            "sweep box-2"
        ]
    );
}

#[test]
fn driver_limit_of_one_queues_despite_a_larger_box_limit() {
    let config = UserConfig {
        drivers: driver_limit(PROVIDER, 1),
        ..box_limit(5)
    };
    let (w, _) = with_live_box(config);
    let v = run(&w.cx, &req("second")).unwrap();
    assert!(v.queued);
    assert_eq!(w.driver.calls(), ["acquire", "exec box-1", "sweep box-1"]);
}

#[test]
fn asleep_box_still_counts_toward_the_limit() {
    let (w, holder) = with_live_box(box_limit(1));
    w.cx.store()
        .unwrap()
        .apply(Decision::new(holder).box_input(BoxInput::Sleep))
        .unwrap();
    let v = run(&w.cx, &req("waiter")).unwrap();
    assert!(v.queued);
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::WaitingForBox);
    assert_eq!(w.driver.calls(), ["acquire", "exec box-1", "sweep box-1"]);
}
