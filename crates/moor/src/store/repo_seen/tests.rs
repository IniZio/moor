//! A repo's Proxy events: only its own Boxes, only the two kinds, only the window.

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::domain::event::Event;
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::BoxId;

use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::cx::box_repo;

fn add(store: &Store, name: &str, with_repo: bool) -> BoxId {
    let nt = NewBox {
        name: crate::testing::box_name(name),
        repo: with_repo.then(box_repo),
        spec: SpecFields::default(),
    };
    store.apply(Decision::insert(nt)).unwrap().box_id
}

fn withheld(shape: &str) -> Event {
    Event::CredentialWithheld {
        host: "api.github.com".into(),
        rule: "github-api".into(),
        method: "GET".into(),
        shape: Some(shape.into()),
        reason: None,
        query: None,
    }
}

#[test]
fn a_repo_reads_the_withheld_events_of_its_own_boxes_in_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock.clone());
    let ours = add(&store, "ours", true);
    let other = add(&store, "other", false);
    store
        .record_proxy_event(ours, &withheld("GET /old"))
        .unwrap();
    clock.sleep(Duration::from_hours(8 * 24));
    store
        .record_proxy_event(ours, &withheld("GET /user"))
        .unwrap();
    store
        .record_proxy_event(other, &withheld("GET /other"))
        .unwrap();
    let read = store.repo_proxy_events(&box_repo().repo, 7).unwrap();
    let rows: Vec<_> = read
        .iter()
        .map(|(id, kind, p)| (*id, kind.as_str(), p.get("shape").cloned()))
        .collect();
    assert_eq!(
        rows,
        [(ours.get(), "credential_withheld", Some("GET /user".into()))]
    );
}
