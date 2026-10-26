//! A Box that sleeps keeps its claim on its machine but frees its slot; waking takes the lowest
//! free slot on the same machine again, or waits while there is none.

use moor_core::config::{MachinePick, RepoMachine, UserMachine};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::user::{Identity, UserId};
use moor_core::{BoxId, BoxName};

use crate::store::tests::open;
use crate::store::{MachineWake, Store};

struct Mini {
    store: Store,
    user: UserId,
    _dir: tempfile::TempDir,
}

impl Mini {
    fn new(capacity: u16) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = open(dir.path());
        let (user, _) = store
            .enroll(&Identity::parse("slack:T1:U1").unwrap())
            .unwrap();
        let mini = UserMachine::parse("mini", "dev@mini", &["macos".to_owned()])
            .and_then(|m| m.with_capacity(capacity))
            .unwrap();
        store.set_user_machine(user, &mini).unwrap();
        Self {
            store,
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

    fn claim(&self, name: &str) -> (BoxId, MachinePick) {
        let id = self.new_box(name);
        let want = RepoMachine {
            labels: vec!["macos".to_owned()],
            ports: vec![8081],
            ..RepoMachine::default()
        };
        (
            id,
            self.store.claim_machine(id, self.user, &want, &[]).unwrap(),
        )
    }

    fn slot_and_ports(&self, id: BoxId) -> Option<(u16, Vec<u16>)> {
        let held = self.store.box_machine(id).unwrap();
        held.map(|h| (h.slot, h.ports))
    }
}

fn slot_of(pick: &MachinePick) -> Option<u16> {
    match pick {
        MachinePick::Attach { slot, .. } => Some(*slot),
        MachinePick::LeftOut { .. } => None,
    }
}

#[test]
fn a_sleeping_box_frees_its_slot_for_another_box() {
    let m = Mini::new(1);
    let (a, _) = m.claim("a");

    assert_eq!(m.store.sleep_machine(a).unwrap().as_deref(), Some("mini"));
    assert_eq!(m.store.sleep_machine(a).unwrap(), None, "already asleep");

    assert_eq!(m.slot_and_ports(a), None, "an asleep Box serves no machine");
    let claim = m.store.claimed_machine(a).unwrap().unwrap();
    assert!(claim.asleep, "the claim on mini is kept");
    let (_, pick) = m.claim("b");
    assert_eq!(slot_of(&pick), Some(0), "{pick:?}");
}

#[test]
fn waking_takes_the_lowest_free_slot_and_moves_the_ports_by_it() {
    let m = Mini::new(3);
    let (a, _) = m.claim("a");
    let (b, _) = m.claim("b");
    let (c, _) = m.claim("c");
    assert_eq!(m.slot_and_ports(c), Some((2, vec![8101])));
    m.store.sleep_machine(a).unwrap();
    m.store.sleep_machine(c).unwrap();

    assert_eq!(m.store.wake_machine(c).unwrap(), MachineWake::Woke);
    assert_eq!(m.slot_and_ports(c), Some((0, vec![8081])));
    assert_eq!(m.store.wake_machine(a).unwrap(), MachineWake::Woke);
    assert_eq!(m.slot_and_ports(a), Some((2, vec![8101])));
    assert_eq!(m.slot_and_ports(b), Some((1, vec![8091])));
    assert_eq!(m.store.wake_machine(a).unwrap(), MachineWake::Nothing);
}

#[test]
fn waking_on_a_full_machine_stays_asleep_and_says_why() {
    let m = Mini::new(1);
    let (a, _) = m.claim("a");
    m.store.sleep_machine(a).unwrap();
    let (b, _) = m.claim("b");

    let wake = m.store.wake_machine(a).unwrap();
    assert!(
        matches!(&wake, MachineWake::Waiting { name, reason } if name == "mini" && reason.contains("Box b")),
        "{wake:?}"
    );
    assert_eq!(m.slot_and_ports(a), None);
    assert!(m.store.claimed_machine(a).unwrap().unwrap().asleep);

    m.store.detach_machine(b).unwrap();
    assert_eq!(m.store.wake_machine(a).unwrap(), MachineWake::Woke);
    assert_eq!(m.slot_and_ports(a), Some((0, vec![8081])));
}

#[test]
fn closing_an_asleep_box_removes_its_claim() {
    let m = Mini::new(1);
    let (a, _) = m.claim("a");
    m.store.sleep_machine(a).unwrap();

    let close = Decision::new(a)
        .lifecycle(BoxLifecycleInput::Close)
        .session(SessionInput::End);
    m.store.apply(close).unwrap();

    assert_eq!(m.store.claimed_machine(a).unwrap(), None);
    assert_eq!(m.store.wake_machine(a).unwrap(), MachineWake::Nothing);
}
