//! The outbox fills with the event that matches, hands each webhook's deliveries out one at a
//! time in order, and records each outcome on the webhook.
#![allow(
    clippy::indexing_slicing,
    reason = "test assertions index claimed rows"
)]

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, NewBox};
use moor_core::domain::event::EVENT_KINDS;
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::user::{Identity, UserId};
use moor_core::{BoxHandle, BoxName, ProviderId, Secret};

use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::vault::Vault;

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    vault: Vault,
    clock: Arc<FakeClock>,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    World {
        _dir: dir,
        store,
        vault,
        clock,
    }
}

impl World {
    fn user(&self, who: &str) -> UserId {
        self.store
            .enroll(&Identity::parse(who).unwrap(), None)
            .unwrap()
            .0
    }

    /// A webhook of `user` on `events`, still waiting for setup.
    fn waiting_webhook(&self, user: UserId, events: &[&str]) -> i64 {
        let events: Vec<String> = events.iter().map(|&k| k.to_owned()).collect();
        let key = Secret::new(b"k".to_vec());
        let row = ("https://example.com/h", events.as_slice(), None);
        self.store
            .add_user_webhook(&self.vault, user, row, &key, 10)
            .unwrap()
            .unwrap()
    }

    /// An active webhook of `user` on `events`.
    fn webhook(&self, user: UserId, events: &[&str]) -> i64 {
        let id = self.waiting_webhook(user, events);
        let store = &self.store;
        assert!(store
            .activate_user_webhook(&self.vault, user, id, None)
            .unwrap());
        id
    }

    /// A Box for `user` that has written three events, and their ids.
    fn new_box(&self, name: &str, user: UserId) -> Vec<i64> {
        let applied = self.store.apply(insert(name).requester(user)).unwrap();
        let mut events = applied.events;
        for to in ["x", "y"] {
            let copied = ExtraEvent::Copied {
                from: "f".to_owned(),
                to: to.to_owned(),
            };
            let d = Decision::new(applied.box_id).event(copied);
            events.extend(self.store.apply(d).unwrap().events);
        }
        events
    }

    fn health(&self, user: UserId) -> (Option<i64>, Option<i64>, Option<String>) {
        let w = self.store.user_webhooks(user).unwrap().remove(0);
        (w.last_ok_at, w.failing_since, w.last_error)
    }
}

fn insert(name: &str) -> Decision {
    Decision::insert(NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    })
}

#[test]
fn an_event_queues_one_delivery_per_matching_webhook_of_the_box_owner() {
    let w = world();
    let (alice, bob) = (w.user("local:2000"), w.user("local:2001"));
    let all = w.webhook(alice, &EVENT_KINDS);
    let _other_kind = w.webhook(alice, &["relaunch_capped"]);
    let _bobs = w.webhook(bob, &EVENT_KINDS);
    let events = w.new_box("a", alice);
    assert!(!events.is_empty());
    let want: Vec<(i64, i64)> = events.iter().map(|&e| (all, e)).collect();
    assert_eq!(w.store.webhook_deliveries().unwrap(), want);
}

#[test]
fn a_webhook_waiting_for_setup_queues_nothing_until_it_is_active() {
    let w = world();
    let alice = w.user("local:2000");
    let id = w.waiting_webhook(alice, &EVENT_KINDS);
    assert!(!w.new_box("a", alice).is_empty());
    assert_eq!(w.store.webhook_deliveries().unwrap(), []);
    assert!(w
        .store
        .activate_user_webhook(&w.vault, alice, id, None)
        .unwrap());
    let later = w.new_box("b", alice);
    let want: Vec<(i64, i64)> = later.iter().map(|&e| (id, e)).collect();
    assert_eq!(w.store.webhook_deliveries().unwrap(), want);
}

#[test]
fn a_box_without_matching_webhooks_queues_nothing_and_a_failed_apply_leaves_nothing() {
    let w = world();
    let alice = w.user("local:2000");
    w.webhook(alice, &["relaunch_capped"]);
    w.new_box("a", alice);
    assert_eq!(w.store.webhook_deliveries().unwrap(), vec![]);
    w.webhook(alice, &EVENT_KINDS);
    let illegal = insert("b")
        .requester(alice)
        .box_input(BoxInput::Acquired {
            handle: BoxHandle::new("h"),
        })
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    assert!(w.store.apply(illegal).is_err());
    assert_eq!(w.store.webhook_deliveries().unwrap(), vec![]);
}

#[test]
fn claims_hand_out_one_delivery_per_webhook_oldest_first_until_the_lease_ends() {
    let w = world();
    let (alice, bob) = (w.user("local:2000"), w.user("local:2001"));
    let (a, b) = (w.webhook(alice, &EVENT_KINDS), w.webhook(bob, &EVENT_KINDS));
    let (ea, eb) = (w.new_box("a", alice), w.new_box("b", bob));
    let lease = Duration::from_millis(5_000);
    let first = w.store.claim_webhook_deliveries(10, lease).unwrap();
    let got: Vec<(i64, String)> = first.iter().map(|d| (d.webhook, d.url.clone())).collect();
    assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), vec![a, b]);
    assert_eq!((first[0].ts, first[0].user), (1_000_000, alice));
    assert!(w
        .store
        .claim_webhook_deliveries(10, lease)
        .unwrap()
        .is_empty());
    w.clock.sleep(Duration::from_millis(5_000));
    let again = w.store.claim_webhook_deliveries(1, lease).unwrap();
    assert_eq!(
        again.iter().map(|d| d.delivery).collect::<Vec<_>>(),
        vec![first[0].delivery]
    );
    assert!(w
        .store
        .report_webhook_delivery(first[0].delivery, Ok(()))
        .unwrap());
    let next = w.store.claim_webhook_deliveries(10, lease).unwrap();
    let kinds_of_a: Vec<i64> = next
        .iter()
        .filter(|d| d.webhook == a)
        .map(|d| d.delivery)
        .collect();
    assert_eq!(kinds_of_a.len(), 1);
    assert!(kinds_of_a[0] > first[0].delivery);
    assert_eq!((ea.len(), eb.len()), (3, 3));
}

#[test]
fn reports_record_health_and_end_the_delivery() {
    let w = world();
    let alice = w.user("local:2000");
    w.webhook(alice, &EVENT_KINDS);
    w.new_box("a", alice);
    let lease = Duration::from_millis(1);
    let take = || {
        w.store
            .claim_webhook_deliveries(1, lease)
            .unwrap()
            .remove(0)
            .delivery
    };
    let one = take();
    assert!(w
        .store
        .report_webhook_delivery(one, Err(&"x".repeat(300)))
        .unwrap());
    assert_eq!(
        w.health(alice),
        (None, Some(1_000_000), Some("x".repeat(200)))
    );
    w.clock.sleep(Duration::from_millis(10));
    let two = take();
    assert!(w.store.report_webhook_delivery(two, Err("503")).unwrap());
    assert_eq!(
        w.health(alice),
        (None, Some(1_000_000), Some("503".to_owned()))
    );
    assert!(!w.store.report_webhook_delivery(two, Ok(())).unwrap());
    let three = take();
    assert!(w.store.report_webhook_delivery(three, Ok(())).unwrap());
    assert_eq!(w.health(alice), (Some(1_000_010), None, None));
}

#[test]
fn removing_a_webhook_drops_its_pending_deliveries() {
    let w = world();
    let alice = w.user("local:2000");
    let (gone, kept) = (
        w.webhook(alice, &EVENT_KINDS),
        w.webhook(alice, &EVENT_KINDS),
    );
    w.new_box("a", alice);
    assert!(w.store.delete_user_webhook(alice, gone).unwrap());
    let left = w.store.webhook_deliveries().unwrap();
    assert!(!left.is_empty() && left.iter().all(|&(webhook, _)| webhook == kept));
}
