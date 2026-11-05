//! What the machine service learns of a Box's slot through the shared egress Proxy's ledger: why
//! a Box holds none, a slot released while the Box sleeps or parks, and the last answer while the
//! controller is down.

use std::sync::{Arc, Mutex};

use moor_core::config::{RepoMachine, UserMachine};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::Driver;
use moor_core::user::Identity;
use moor_core::{BoxId, HoldOp, HoldValue};
use moor_schema::tables::UserMachinesV2;
use sea_query::{Expr, ExprTrait as _, Query};

use super::{box_ledger, OnLedger};
use crate::proxy::front::Hold;
use crate::proxy::keeper::tests::ready_box;
use crate::proxy::keeper::{Deps, KEEPERS};
use crate::store::db::DbExt as _;
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;

/// A store with one ready Box and its ledger, as the shared egress Proxy reaches it.
struct Rig {
    dir: tempfile::TempDir,
    store: Store,
    driver: Arc<FakeDriver>,
    id: BoxId,
    ledger: OnLedger,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let driver = Arc::new(FakeDriver::new());
    let id = ready_box(&store, &driver);
    let ledger = box_ledger(&store, id);
    Rig {
        dir,
        store,
        driver,
        id,
        ledger,
    }
}

impl Rig {
    /// The Box's requester holds a slot on the machine `mini`, forwarding Box port 8081.
    fn hold_mini(&self) {
        let user = self.user();
        let mini = UserMachine::parse("mini", &["macos".to_owned()]).unwrap();
        self.store.set_user_machine(user, &mini).unwrap();
        let held = self
            .store
            .hold_machine(self.id, user, &HoldOp::repo(), Some(&want()), &[])
            .unwrap();
        assert!(matches!(held, HoldValue::Held { .. }), "{held:?}");
    }

    fn user(&self) -> moor_core::user::UserId {
        let who = Identity::parse("slack:T1:U1").unwrap();
        self.store.enroll(&who, None).unwrap().0
    }

    fn missing(&self) -> String {
        match self.ledger.hold().unwrap() {
            Hold::Missing { reason, .. } => reason,
            Hold::Held(h) => unreachable!("held {}", h.name),
        }
    }

    /// A keeper for the Box that never ticks on its own.
    fn keeper(&self) -> Arc<crate::proxy::keeper::Kept> {
        let deps = Deps {
            store: self.store.clone(),
            driver: Arc::clone(&self.driver) as Arc<dyn Driver>,
            clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
            egress: None,
            runner: self.dir.path().join("runner.sock"),
            log: Arc::new(Mutex::new(std::io::sink())),
        };
        KEEPERS.hold(&Arc::new(deps), self.id, 41_000)
    }
}

fn want() -> RepoMachine {
    RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081],
        ..RepoMachine::default()
    }
}

const RELEASED: &str = "machine mini is released while this Box sleeps; try again later";
const NO_SLOT: &str = "machine mini has no free slot yet; this Box takes one as soon as another Box frees it, so try again later";

#[test]
fn a_box_without_a_claim_is_told_to_call_machine_claim() {
    let r = rig();
    assert_eq!(
        r.missing(),
        "this Box has no machine yet; call the machine_claim tool to take a slot on one of your user's machines"
    );
}

#[test]
fn an_asleep_claim_is_released_while_parked_and_waits_once_asked_up() {
    let r = rig();
    r.hold_mini();
    assert!(matches!(r.ledger.hold().unwrap(), Hold::Held(h) if h.name == "mini"));
    let kept = r.keeper();
    kept.park();
    assert_eq!(r.missing(), RELEASED, "parked before the machine sleeps");
    r.store
        .apply(Decision::new(r.id).session(SessionInput::AgentQuiet))
        .unwrap();
    let sleep = Decision::new(r.id)
        .lifecycle(BoxLifecycleInput::Sleep {
            auto: false,
            reason: None,
        })
        .box_input(BoxInput::Sleep);
    r.store.apply(sleep).unwrap();
    assert_eq!(r.missing(), RELEASED, "asleep");
    r.store
        .apply(Decision::new(r.id).box_input(BoxInput::Wake))
        .unwrap();
    kept.restore().unwrap();
    assert_eq!(r.missing(), NO_SLOT, "awake, before the keeper's next tick");
    KEEPERS.end(r.id);
}

#[test]
fn a_removed_machine_is_refused() {
    let r = rig();
    r.hold_mini();
    let machine_id = match r.ledger.hold().unwrap() {
        Hold::Held(h) => h.machine_id,
        Hold::Missing { reason, .. } => unreachable!("not held: {reason}"),
    };
    r.store
        .writer()
        .run(
            Query::delete()
                .from_table(UserMachinesV2::Table)
                .and_where(Expr::col(UserMachinesV2::MachineId).eq(machine_id))
                .take(),
        )
        .unwrap();
    let held = r
        .store
        .hold_machine(r.id, r.user(), &HoldOp::repo(), Some(&want()), &[])
        .unwrap();
    let removed = HoldValue::Refused {
        reason: "machine mini was removed".to_owned(),
    };
    assert_eq!(held, removed);
    assert_eq!(r.missing(), "machine mini was removed");
}

#[test]
fn a_held_claim_stays_held_while_the_controller_is_down() {
    let r = rig();
    r.hold_mini();
    assert!(matches!(r.ledger.hold().unwrap(), Hold::Held(h) if h.name == "mini"));
    r.ledger.down();
    assert!(matches!(r.ledger.hold().unwrap(), Hold::Held(h) if h.name == "mini"));
}
