//! A Box claims one of its user's machines through `hold_machine`; waiters take freed slots in
//! the order they asked, through `grant_machine` on each supervisor tick.

use std::sync::Arc;
use std::time::Duration;

use moor_core::config::{RepoMachine, UserMachine};
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::user::{Identity, UserId};
use moor_core::view::MachineHoldState;
use moor_core::{BoxId, BoxName};

use crate::store::tests::open;
use moor_core::{HoldOp, HoldValue};

use crate::store::{MachineWake, Store};
use crate::testing::clock::FakeClock;

struct Rig {
    store: Store,
    clock: Arc<FakeClock>,
    user: UserId,
    _dir: tempfile::TempDir,
}

impl Rig {
    fn new(capacity: u16) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (store, clock) = open(dir.path());
        let user = enroll(&store, "slack:T1:U1");
        add_machine(&store, user, "mini", capacity);
        Self {
            store,
            clock,
            user,
            _dir: dir,
        }
    }

    fn new_box(&self, name: &str) -> BoxId {
        let nt = NewBox {
            name: BoxName::parse_allow_bare(name).unwrap(),
            repo: None,
        };
        self.store.apply(Decision::insert(nt)).unwrap().box_id
    }

    fn claim_as(&self, id: BoxId, user: UserId, op: &HoldOp) -> HoldValue {
        self.store
            .hold_machine(id, user, op, Some(&repo()), &[])
            .unwrap()
    }

    fn claim(&self, id: BoxId, wait: Option<Duration>) -> HoldValue {
        let wait_until = wait.map(|d| self.clock.now().saturating_add(d));
        self.claim_as(id, self.user, &claim_op(None, wait_until))
    }

    fn slot_of(&self, id: BoxId) -> Option<u16> {
        self.store.box_machine(id).unwrap().map(|h| h.slot)
    }

    fn has_row(&self, id: BoxId) -> bool {
        self.store.claimed_machine(id).unwrap().is_some()
    }
}

fn enroll(store: &Store, who: &str) -> UserId {
    store
        .enroll(&Identity::parse(who).unwrap(), None)
        .unwrap()
        .0
}

fn add_machine(store: &Store, user: UserId, name: &str, capacity: u16) {
    let m = UserMachine::parse(name, &format!("dev@{name}"), &["macos".to_owned()])
        .and_then(|m| m.with_capacity(capacity))
        .unwrap();
    store.set_user_machine(user, &m).unwrap();
}

fn repo() -> RepoMachine {
    RepoMachine {
        labels: vec!["macos".to_owned()],
        ports: vec![8081],
        ..RepoMachine::default()
    }
}

fn claim_op(machine: Option<&str>, wait_until: Option<Ts>) -> HoldOp {
    HoldOp::Claim {
        labels: vec!["macos".to_owned()],
        machine: machine.map(str::to_owned),
        ports: None,
        wait_until,
    }
}

fn waiting(v: &HoldValue) -> Option<u16> {
    match v {
        HoldValue::Waiting { position, .. } => Some(*position),
        _ => None,
    }
}

fn held_slot(v: &HoldValue) -> Option<u16> {
    match v {
        HoldValue::Held { slot, .. } => Some(*slot),
        _ => None,
    }
}

#[test]
fn a_claim_on_a_free_machine_holds_slot_zero_with_the_repo_ports() {
    let r = Rig::new(2);
    let a = r.new_box("a");

    let got = r.claim(a, None);

    let want = HoldValue::Held {
        machine: "mini".to_owned(),
        ssh: "dev@mini".to_owned(),
        machine_id: None,
        slot: 0,
        ports: vec![8081],
    };
    assert_eq!(got, want);
    assert_eq!(
        r.claim(a, None),
        want,
        "claiming again gives the same answer"
    );
    assert_eq!(r.slot_of(a), Some(0));
}

#[test]
fn a_claim_without_a_wait_on_a_full_machine_is_refused_naming_the_holders() {
    let r = Rig::new(1);
    let (a, b) = (r.new_box("a"), r.new_box("b"));
    r.claim(a, None);

    let got = r.claim(b, None);

    assert!(
        matches!(&got, HoldValue::Refused { reason } if reason.contains("Box a")),
        "{got:?}"
    );
    assert!(!r.has_row(b), "a refused claim records nothing");
    assert_eq!(r.slot_of(a), Some(0));
}

#[test]
fn the_older_of_two_waiters_takes_the_freed_slot() {
    let r = Rig::new(1);
    let (a, b, c) = (r.new_box("a"), r.new_box("b"), r.new_box("c"));
    r.claim(a, None);
    let hour = Some(Duration::from_secs(3600));
    let first = r.claim(b, hour);
    r.clock.sleep(Duration::from_secs(1));
    let second = r.claim(c, hour);
    assert_eq!(waiting(&first), Some(1), "{first:?}");
    assert_eq!(waiting(&second), Some(2), "{second:?}");
    let HoldValue::Waiting { reason, .. } = first else {
        unreachable!()
    };
    assert!(reason.contains("Box a"), "{reason}");

    r.claim_as(a, r.user, &HoldOp::Release);
    assert!(matches!(
        r.store.grant_machine(c).unwrap(),
        MachineWake::Waiting { .. }
    ));

    assert_eq!(r.slot_of(b), Some(0), "the older waiter holds the slot");
    assert_eq!(r.slot_of(c), None);
    assert_eq!(r.store.grant_machine(b).unwrap(), MachineWake::Woke);
}

#[test]
fn an_expired_want_is_dropped_and_the_slot_goes_to_the_next_waiter() {
    let r = Rig::new(1);
    let (a, b, c) = (r.new_box("a"), r.new_box("b"), r.new_box("c"));
    r.claim(a, None);
    r.claim(b, Some(Duration::from_secs(10)));
    r.clock.sleep(Duration::from_secs(1));
    r.claim(c, Some(Duration::from_secs(3600)));
    r.clock.sleep(Duration::from_secs(60));

    r.claim_as(a, r.user, &HoldOp::Release);
    assert_eq!(r.store.grant_machine(c).unwrap(), MachineWake::Woke);

    assert_eq!(r.slot_of(c), Some(0));
    assert!(!r.has_row(b), "the expired want is gone");
    assert_eq!(r.store.grant_machine(b).unwrap(), MachineWake::Nothing);
}

#[test]
fn release_frees_the_slot_for_another_box() {
    let r = Rig::new(1);
    let (a, b) = (r.new_box("a"), r.new_box("b"));
    r.claim(a, None);

    let got = r.claim_as(a, r.user, &HoldOp::Release);

    assert_eq!(
        got,
        HoldValue::Released {
            machine: Some("mini".to_owned())
        }
    );
    assert!(!r.has_row(a));
    assert_eq!(held_slot(&r.claim(b, None)), Some(0));
}

#[test]
fn sleep_frees_the_slot_and_a_wake_takes_one_again_through_the_grant() {
    let r = Rig::new(2);
    let (a, b, c) = (r.new_box("a"), r.new_box("b"), r.new_box("c"));
    r.claim(a, None);
    r.claim(b, None);
    assert_eq!(r.store.sleep_machine(a).unwrap().as_deref(), Some("mini"));
    let claim = r.store.claimed_machine(a).unwrap().unwrap();
    assert_eq!(claim.state, MachineHoldState::Asleep);
    assert_eq!((claim.slot, claim.ports), (0, vec![8081]));
    assert_eq!(r.slot_of(a), None, "an asleep Box serves no machine");
    assert_eq!(held_slot(&r.claim(c, None)), Some(0), "the freed slot");
    r.claim_as(b, r.user, &HoldOp::Release);

    assert_eq!(r.store.grant_machine(a).unwrap(), MachineWake::Woke);

    let held = r.store.box_machine(a).unwrap().unwrap();
    assert_eq!((held.slot, held.ports), (1, vec![8091]), "a different slot");
    assert_eq!(
        r.store.grant_machine(a).unwrap(),
        MachineWake::Woke,
        "still held"
    );
}

#[test]
fn a_box_cannot_name_another_users_machine() {
    let r = Rig::new(1);
    let other = enroll(&r.store, "slack:T1:U2");
    add_machine(&r.store, other, "studio", 1);
    let a = r.new_box("a");

    let got = r.claim_as(a, r.user, &claim_op(Some("studio"), None));

    assert!(
        matches!(&got, HoldValue::Refused { reason } if reason.contains("studio")),
        "{got:?}"
    );
    assert!(!r.has_row(a));
    let theirs = r.new_box("b");
    let mine = r.claim_as(theirs, other, &claim_op(Some("studio"), None));
    assert_eq!(held_slot(&mine), Some(0), "its owner still gets it");
}
