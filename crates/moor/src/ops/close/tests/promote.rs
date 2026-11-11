use crate::store::db::DbExt as _;
use moor_schema::tables::{BoxState, Boxes};
use sea_query::{Expr, ExprTrait as _, Query};
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use super::is_working;

use moor_core::config::{NodeConfig, UserConfig};
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{Decision, Field, HandsFields, NewBox, SpecFields};
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxHandle, BoxId, BoxName, BoxPath, CloseReq, ProviderId, ShowReq, StartReq};

use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::ops::close::run as close;
use crate::ops::common::promote::{pass, Promoting, PROMOTE_WAIT};
use crate::ops::show;
use crate::ops::start;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::{repo_at, FakeForge};
use crate::testing::proc::ScriptedProc;

mod nodes;

const PROVIDER: &str = "local-container";
const HOLDER_BOX: &str = "box-1";
const HOLDER_WORKDIR: &str = "/work/box-1";

struct World {
    cx: Cx,
    driver: Arc<FakeDriver>,
    forge: Arc<FakeForge>,
    holder: BoxId,
    _dir: tempfile::TempDir,
}

fn limit(n: u32) -> UserConfig {
    UserConfig {
        nodes: BTreeMap::from([(
            PROVIDER.to_owned(),
            NodeConfig {
                runtime: PROVIDER.to_owned(),
                labels: Vec::new(),
                max_boxes: NonZeroU32::new(n),
            },
        )]),
        ..UserConfig::default()
    }
}

/// A live machine held by Box `holder`, bound to `config` and `driver`.
fn world(config: UserConfig, driver: FakeDriver) -> World {
    world_failing(config, driver, None)
}

/// Like [`world`]; the next acquire after the holder's fails with `acquire_err`.
fn world_failing(
    config: UserConfig,
    driver: FakeDriver,
    acquire_err: Option<DriverError>,
) -> World {
    let dir = tempfile::tempdir().unwrap();
    // The holder is the driver's first Box, so close probes `box-1` at its recorded workdir.
    let probe = BoxPath::under(HOLDER_WORKDIR, ".moor/boxwork").unwrap();
    let driver = driver.peek_text(HOLDER_BOX, &probe, "ahead=0 dirty=0");
    let handle = acquire_holder(&driver);
    let driver = Arc::new(match acquire_err {
        Some(e) => driver.fail_acquire(e),
        None => driver,
    });
    let forge = Arc::new(FakeForge::new());
    let cx = Cx::fake_with(
        dir.path(),
        driver.clone(),
        Arc::new(ScriptedProc::new()),
        config,
    )
    .with_forge(forge.clone());
    let holder = insert_holder(&cx, handle);
    World {
        cx,
        driver,
        forge,
        holder,
        _dir: dir,
    }
}

/// Acquires the holder's Box, so the driver knows it and can run the close probe in it.
fn acquire_holder(driver: &FakeDriver) -> BoxHandle {
    let got = driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider: ProviderId::new(PROVIDER),
            box_id: BoxId::new(1),
            name: BoxName::parse_allow_bare("holder").unwrap(),
            class: None,
            nonce: "n".to_owned(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    assert_eq!(got.handle.as_str(), HOLDER_BOX);
    got.handle
}

fn insert_holder(cx: &Cx, handle: BoxHandle) -> BoxId {
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare("holder").unwrap(),
            repo: None,
            spec: SpecFields::default(),
        }))
        .unwrap()
        .box_id;
    let fields = HandsFields {
        provider: Field::Set(ProviderId::new(PROVIDER)),
        workdir: Field::Set(HOLDER_WORKDIR.into()),
        ..HandsFields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(HandsInc::FIRST, fields);
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .hands_input(HandsInc::FIRST, HandsInput::Acquired { handle });
    store.apply(d).unwrap();
    on_provider_node(cx, id, PROVIDER);
    id
}

/// Records `id` on the node a config without `nodes:` builds from `provider`.
fn on_provider_node(cx: &Cx, id: BoxId, provider: &str) {
    cx.store()
        .unwrap()
        .set_box_node(id, moor_core::name::NodeName::parse(provider).ok().as_ref())
        .unwrap();
}

fn start_req(name: &str) -> StartReq {
    StartReq {
        name: BoxName::parse_allow_bare(name).unwrap(),
        prompt: None,
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: false,
        cold: false,
        resume: false,
        repo: None,
        parent: None,
        also: None,
        identity: None,
    }
}

/// Queues `name` through `start` at the limit.
fn queue(w: &World, name: &str) -> BoxId {
    queue_req(w, &start_req(name))
}

/// Queues `req` through `start` at the limit.
fn queue_req(w: &World, req: &StartReq) -> BoxId {
    let v = start::run(&w.cx, req).unwrap();
    assert!(v.queued, "{} should queue", req.name.as_str());
    BoxId::new(v.boxed.id)
}

/// A waiting Box made without consulting any limit.
fn waiting_directly(w: &World, name: &str) -> BoxId {
    let store = w.cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: BoxName::parse_allow_bare(name).unwrap(),
            repo: None,
            spec: SpecFields::default(),
        }))
        .unwrap()
        .box_id;
    let d = Decision::new(id).lifecycle(BoxLifecycleInput::AtCapacity { max_boxes: 1 });
    store.apply(d).unwrap();
    id
}

fn close_holder(w: &World) -> moor_core::view::CloseView {
    let req = CloseReq {
        target: BoxName::parse_allow_bare("holder").unwrap().into(),
        force: false,
    };
    close(&w.cx, &req).unwrap()
}

fn row_of(w: &World, id: BoxId) -> crate::store::BoxRow {
    w.cx.store().unwrap().box_row(id).unwrap().unwrap()
}

fn lifecycle_of(w: &World, id: BoxId) -> BoxLifecycle {
    row_of(w, id).lifecycle
}

fn kinds(w: &World, id: BoxId) -> Vec<String> {
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    events.into_iter().map(|e| e.kind).collect()
}

fn acquires(w: &World) -> usize {
    // The holder's own acquire is not counted.
    w.driver.calls().iter().filter(|c| *c == "acquire").count() - 1
}

fn warning_codes(w: &World) -> Vec<String> {
    w.cx.take_warnings().into_iter().map(|x| x.code).collect()
}

/// Runs the controller's promotion pass once.
fn promote_pass(w: &World) {
    for promoting in start_pass(w) {
        for warning in promoting.join().unwrap() {
            w.cx.warn(warning);
        }
    }
}

/// Starts the promotion pass once without waiting for the promotions it starts.
fn start_pass(w: &World) -> Vec<Promoting> {
    pass(&w.cx, w.cx.store().unwrap(), &|| false).unwrap()
}

/// A Box on the provider's node that holds an acquired machine and is setting up.
fn setting_up_holding(w: &World, name: &str) -> BoxId {
    let store = w.cx.store().unwrap();
    let name = BoxName::parse_allow_bare(name).unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: None,
            spec: SpecFields::default(),
        }))
        .unwrap()
        .box_id;
    let got = w
        .driver
        .acquire(&AcquireReq {
            pool_machine: None,
            provider: ProviderId::new(PROVIDER),
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            cache: moor_core::cache::CachePlan::none(),
        })
        .unwrap();
    let fields = HandsFields {
        provider: Field::Set(ProviderId::new(PROVIDER)),
        ..HandsFields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .hands_input(HandsInc::FIRST, HandsInput::Requested)
        .hands(HandsInc::FIRST, fields);
    store.apply(d).unwrap();
    let d =
        Decision::new(id).hands_input(HandsInc::FIRST, HandsInput::Acquired { handle: got.handle });
    store.apply(d).unwrap();
    on_provider_node(&w.cx, id, PROVIDER);
    id
}

fn setup_failed() -> BoxLifecycleInput {
    BoxLifecycleInput::SetupFailed {
        cause: "clone failed".to_owned(),
        which: None,
    }
}

/// Closes the holder, then runs the promotion pass that fills the slot it freed.
fn free_slot(w: &World) -> moor_core::view::CloseView {
    let v = close_holder(w);
    promote_pass(w);
    v
}

#[test]
fn close_failed_closes_every_failed_box_without_setting_up_the_waiter() {
    let w = world(limit(3), FakeDriver::new());
    let store = w.cx.store().unwrap();
    let failed = [setting_up_holding(&w, "one"), setting_up_holding(&w, "two")];
    for id in failed {
        store
            .apply(Decision::new(id).lifecycle(setup_failed()))
            .unwrap();
    }
    let waiter = queue(&w, "waiter");
    let before = acquires(&w);
    let req = moor_core::request::CloseFailedReq { force: false };
    let v = crate::ops::close_failed::run(&w.cx, &req).unwrap();
    assert_eq!(v.closed.len(), 2, "{v:?}");
    assert_eq!(lifecycle_of(&w, waiter), BoxLifecycle::Pending);
    assert_eq!(acquires(&w), before);
    promote_pass(&w);
    assert!(is_working(&row_of(&w, waiter)));
}

#[test]
fn a_waiter_is_promoted_once_the_box_ahead_fails_setup_and_gives_up_its_machine() {
    let w = world(limit(2), FakeDriver::new());
    let ahead = setting_up_holding(&w, "ahead");
    let waiter = queue(&w, "waiter");
    let gone = Decision::new(ahead)
        .lifecycle(setup_failed())
        .hands_input(HandsInc::FIRST, HandsInput::Released);
    w.cx.store().unwrap().apply(gone).unwrap();
    promote_pass(&w);
    assert!(is_working(&row_of(&w, waiter)));
}

#[test]
fn a_freed_slot_promotes_the_oldest_waiter_of_the_provider() {
    let w = world(limit(1), FakeDriver::new());
    let first = queue(&w, "first");
    let second = queue(&w, "second");
    let v = free_slot(&w);
    assert!(v.box_deleted);
    assert!(is_working(&row_of(&w, first)));
    assert_eq!(lifecycle_of(&w, second), BoxLifecycle::Pending);
}

#[test]
fn a_younger_waiter_stays_queued_when_one_slot_frees() {
    let w = world(limit(1), FakeDriver::new());
    let first = queue(&w, "first");
    let second = queue(&w, "second");
    free_slot(&w);
    assert_ne!(lifecycle_of(&w, first), BoxLifecycle::Pending);
    assert_eq!(lifecycle_of(&w, second), BoxLifecycle::Pending);
    assert_eq!(acquires(&w), 1);
}

#[test]
fn promoted_box_goes_through_setting_up_and_gets_a_ready_box() {
    let w = world(limit(1), FakeDriver::new());
    let id = queue(&w, "first");
    free_slot(&w);
    let seen = kinds(&w, id);
    let at = |k: &str| seen.iter().position(|s| s == k);
    assert!(at("pending") < at("setting_up"), "{seen:?}");
    assert!(at("setting_up") < at("working"), "{seen:?}");
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    assert!(is_working(&row));
    let b = row.hands.expect("promoted Box has a machine row");
    assert_eq!(b.state, HandsState::Ready);
    assert!(b.handle.is_some());
    // The holder's acquire, two close probes and report collection come first; each is one exec and one peek,
    // and the close collects artifacts with one more exec.
    assert_eq!(
        w.driver.calls(),
        [
            "acquire".to_owned(),
            format!("exec {HOLDER_BOX}"),
            format!("peek_file {HOLDER_BOX}"),
            format!("exec {HOLDER_BOX}"),
            format!("peek_file {HOLDER_BOX}"),
            format!("exec {HOLDER_BOX}"),
            format!("peek_file {HOLDER_BOX}"),
            format!("exec {HOLDER_BOX}"),
            format!("release {HOLDER_BOX}"),
            // The promoted Box has no repo origin, setup or author, so it runs no exec; it only
            // looks once for its new guest.
            "acquire".into(),
            "peek_file box-2".into(),
        ]
    );
}

#[test]
fn a_waiter_on_a_provider_without_reverse_tunnel_is_not_provisioned() {
    let w = world(limit(1), FakeDriver::new().without_reverse_tunnel_cap());
    let id = waiting_directly(&w, "first");
    free_slot(&w);
    assert_eq!(acquires(&w), 0);
    assert!(!is_working(&row_of(&w, id)));
}

#[test]
fn a_failed_acquire_of_the_promoted_box_is_a_warning() {
    let failure = DriverError::Failed {
        summary: "no capacity".into(),
    };
    let w = world_failing(limit(1), FakeDriver::new(), Some(failure));
    let id = queue(&w, "first");
    let v = free_slot(&w);
    assert!(v.box_deleted);
    assert!(warning_codes(&w).contains(&"promote_failed".to_owned()));
    assert!(!is_working(&row_of(&w, id)));
}

#[test]
fn show_does_not_promote_a_waiter_when_a_slot_is_free() {
    let w = world(limit(1), FakeDriver::new());
    let id = queue(&w, "first");
    w.cx.store()
        .unwrap()
        .apply(Decision::new(w.holder).hands_input(HandsInc::FIRST, HandsInput::DriverMissing))
        .unwrap();
    let req = ShowReq {
        target: None,
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    };
    show::run(&w.cx, &req).unwrap();
    assert_eq!(lifecycle_of(&w, id), BoxLifecycle::Pending);
    assert_eq!(acquires(&w), 0);
}

#[test]
fn nothing_is_promoted_without_a_configured_limit() {
    let w = world(UserConfig::default(), FakeDriver::new());
    let id = waiting_directly(&w, "first");
    free_slot(&w);
    assert_eq!(lifecycle_of(&w, id), BoxLifecycle::Pending);
    assert_eq!(acquires(&w), 0);
}

#[test]
fn a_freed_slot_with_no_waiting_boxes_promotes_nothing() {
    let w = world(limit(1), FakeDriver::new());
    let v = free_slot(&w);
    assert!(v.box_deleted);
    assert_eq!(acquires(&w), 0);
    assert!(warning_codes(&w).is_empty());
}

#[test]
fn a_waiter_whose_box_lock_is_held_is_skipped_with_a_warning() {
    let w = world(limit(1), FakeDriver::new());
    let id = queue(&w, "first");
    let _held = w.cx.owners_arc().hold(id, Cmd::Wake);
    let from = w.cx.clock().instant();
    let v = free_slot(&w);
    let waited = w.cx.clock().instant() - from;
    assert!(waited >= PROMOTE_WAIT, "{waited:?}");
    assert!(waited < PROMOTE_WAIT * 2, "{waited:?}");
    assert!(v.box_deleted);
    let warnings = w.cx.take_warnings();
    let skipped = warnings.iter().find(|x| x.code == "promote_skipped");
    let message = &skipped.expect("promote_skipped warning").message;
    assert!(message.starts_with("cannot promote first:"), "{message}");
    assert_eq!(lifecycle_of(&w, id), BoxLifecycle::Pending);
    assert_eq!(acquires(&w), 0);
}

#[test]
fn a_store_read_error_during_promotion_is_a_warning() {
    let w = world(limit(1), FakeDriver::new());
    queue(&w, "first");
    let db = crate::store::db::Conn::open_file(&w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    db.run(
        Query::update()
            .table(Boxes::Table)
            .value(Boxes::Name, "")
            .and_where(
                Expr::col(Boxes::Id).in_subquery(
                    Query::select()
                        .column(BoxState::Box)
                        .from(BoxState::Table)
                        .and_where(Expr::col(BoxState::State).eq("pending"))
                        .to_owned(),
                ),
            ),
    )
    .unwrap();
    let v = free_slot(&w);
    assert!(v.box_deleted);
    assert!(warning_codes(&w).contains(&"promote_failed".to_owned()));
}

#[test]
fn promoted_box_runs_the_repo_setup_commands_in_its_new_box() {
    // Setup runs through the Proxy, which stages its credentials under the Box user's HOME.
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let driver = FakeDriver::new().peek_text("box-2", &environ, "HOME=/home/box\0");
    let w = world(limit(1), driver);
    let repo = ForgeRepo::new("o", "n");
    let yaml = "setup: [\"echo one\", \"echo two\"]\n";
    w.forge
        .answer(repo.clone(), Ok(repo_at("abc123", Some(yaml))));
    let id = queue_req(
        &w,
        &StartReq {
            repo: Some(repo),
            ..start_req("first")
        },
    );
    free_slot(&w);
    assert!(is_working(&row_of(&w, id)));
    let handle = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    let handle = handle.hands.unwrap().handle.expect("promoted Box handle");
    let argvs: Vec<_> = w
        .driver
        .execs()
        .into_iter()
        .filter(|(h, spec, _)| *h == handle && spec.argv != ["git", "rev-parse", "--git-dir"])
        .map(|(_, spec, _)| spec.argv)
        .collect();
    let ran = argvs
        .iter()
        .filter(|a| {
            a.last()
                .is_some_and(|l| l.contains("moor setup: $ echo one"))
        })
        .count();
    assert_eq!(ran, 1, "{argvs:?}");
}

/// Runs the update `q` against the world's store.
fn exec_on(w: &World, q: &mut sea_query::UpdateStatement) {
    let db = crate::store::db::Conn::open_file(&w.cx.state_dir().unwrap().join("moor.db")).unwrap();
    db.run(q).unwrap();
}

/// Queues `first` from a repo whose stored `.moor.yaml` no longer parses.
fn queue_unparsable(w: &World) -> BoxId {
    let repo = ForgeRepo::new("o", "n");
    w.forge
        .answer(repo.clone(), Ok(repo_at("abc123", Some("setup: []\n"))));
    let req = StartReq {
        repo: Some(repo),
        ..start_req("first")
    };
    let id = queue_req(w, &req);
    exec_on(
        w,
        Query::update()
            .table(Boxes::Table)
            .value(Boxes::RepoConfig, "setup: [")
            .and_where(Expr::col(Boxes::Id).eq(id.get())),
    );
    id
}

/// The `promote_skipped` messages naming `first`.
fn skipped_first(w: &World) -> Vec<String> {
    w.cx.take_warnings()
        .into_iter()
        .filter(|x| x.code == "promote_skipped" && x.message.contains("first"))
        .map(|x| x.message)
        .collect()
}

#[test]
fn a_waiter_whose_settings_no_longer_parse_is_skipped_with_a_warning() {
    let w = world(limit(1), FakeDriver::new());
    let id = queue_unparsable(&w);
    free_slot(&w);
    let skipped = skipped_first(&w);
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(lifecycle_of(&w, id), BoxLifecycle::Pending);
}
