//! The settle pass closes a Box that has stayed `setup_failed` past the grace period.

use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::{BoxId, BoxName, ProviderId};

use crate::cx::Cx;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;
use crate::Moor;

const DAY: Duration = Duration::from_hours(24);

struct World {
    _dir: tempfile::TempDir,
    moor: Moor,
    clock: Arc<FakeClock>,
    driver: Arc<FakeDriver>,
}

fn world(driver: FakeDriver) -> World {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    let cx = Cx::fake_with_clock(
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
        UserConfig::default(),
        clock.clone(),
    );
    World {
        _dir: dir,
        moor: Moor::from_cx(cx),
        clock,
        driver,
    }
}

impl World {
    /// A `setup_failed` Box `n`; `failed_at` is when setup failed, `machine` gives it a machine.
    fn setup_failed(&self, n: &str, failed_at: Option<i64>, machine: bool) -> BoxId {
        let store = self.moor.cx.store().unwrap();
        let name = BoxName::parse_allow_bare(n).unwrap();
        let id = store
            .apply(Decision::insert(NewBox {
                name: name.clone(),
                repo: None,
            }))
            .unwrap()
            .box_id;
        store
            .apply(
                Decision::new(id)
                    .lifecycle(BoxLifecycleInput::Admitted)
                    .box_input(BoxInput::Requested)
                    .set(Fields {
                        box_provider: Field::Set(ProviderId::new("p")),
                        ..Fields::default()
                    }),
            )
            .unwrap();
        if machine {
            let got = self
                .driver
                .acquire(&AcquireReq {
                    pool_machine: None,
                    provider: ProviderId::new("p"),
                    box_id: id,
                    name,
                    class: None,
                    nonce: "n".to_owned(),
                    cache: moor_core::cache::CachePlan::none(),
                })
                .unwrap();
            store
                .apply(Decision::new(id).box_input(BoxInput::Acquired { handle: got.handle }))
                .unwrap();
        }
        store
            .apply(
                Decision::new(id)
                    .lifecycle(BoxLifecycleInput::SetupFailed {
                        cause: "clone failed".to_owned(),
                        which: None,
                    })
                    .set(Fields {
                        setup_failed_at: failed_at.map_or(Field::Clear, Field::Set),
                        ..Fields::default()
                    }),
            )
            .unwrap();
        id
    }

    fn lifecycle(&self, id: BoxId) -> BoxLifecycle {
        let row = self.moor.cx.store().unwrap().box_row(id).unwrap().unwrap();
        row.lifecycle
    }

    fn pass(&self, after: Option<Duration>) -> Vec<String> {
        self.moor.settle_open_closing(&|| false, after)
    }

    fn closed_payload(&self, id: BoxId) -> serde_json::Value {
        let events = self.moor.cx.store().unwrap().events(id, 100).unwrap();
        events
            .iter()
            .find(|e| e.kind == "closed")
            .expect("a closed event")
            .payload
            .clone()
    }
}

#[test]
fn a_box_inside_the_grace_period_stays_open() {
    let w = world(FakeDriver::new());
    let now = w.moor.cx.clock().now().as_millis();
    let id = w.setup_failed("a", Some(now), false);
    w.clock.sleep(DAY.saturating_sub(Duration::from_secs(1)));
    assert!(w.pass(Some(DAY)).is_empty());
    assert_eq!(w.lifecycle(id), BoxLifecycle::SetupFailed);
}

#[test]
fn a_box_past_the_grace_period_closes_with_the_expiry_reason() {
    let w = world(FakeDriver::new());
    let now = w.moor.cx.clock().now().as_millis();
    let id = w.setup_failed("a", Some(now), true);
    w.clock.sleep(DAY);
    assert!(w.pass(Some(DAY)).is_empty());
    assert_eq!(w.lifecycle(id), BoxLifecycle::Closed);
    assert_eq!(
        w.closed_payload(id),
        serde_json::json!({"from": "setup_failed", "reason": "setup_failed_expired"})
    );
}

#[test]
fn a_zero_grace_period_never_closes() {
    let w = world(FakeDriver::new());
    let id = w.setup_failed("a", Some(0), false);
    w.clock.sleep(DAY * 30);
    assert!(w.pass(None).is_empty());
    assert_eq!(w.lifecycle(id), BoxLifecycle::SetupFailed);
}

#[test]
fn a_setup_failed_box_with_no_failure_time_is_skipped() {
    let w = world(FakeDriver::new());
    let id = w.setup_failed("a", None, false);
    w.clock.sleep(DAY * 30);
    assert!(w.pass(Some(DAY)).is_empty());
    assert_eq!(w.lifecycle(id), BoxLifecycle::SetupFailed);
}

#[test]
fn a_box_that_will_not_close_is_reported_once_per_pass() {
    let w = world(FakeDriver::new().fail_release_always(DriverError::Failed {
        summary: "stuck".into(),
    }));
    let id = w.setup_failed("a", Some(0), true);
    w.clock.sleep(DAY);
    let first = w.pass(Some(DAY));
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first
        .iter()
        .all(|m| m.contains("Box a") && m.contains("stuck")));
    assert_eq!(w.lifecycle(id), BoxLifecycle::SetupFailed);
    assert_eq!(w.pass(Some(DAY)).len(), 1);
}
