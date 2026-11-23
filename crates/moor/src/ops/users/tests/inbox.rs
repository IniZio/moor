//! A person clears only their own inbox items, whatever ids they send.
#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use moor_core::domain::decision::{Decision, ExtraEvent, NewBox, SpecFields};
use moor_core::inbox::InboxValue;
use moor_core::request::{InboxClear, InboxOp, UserCmd};
use moor_core::user::Identity;
use moor_core::BoxName;

use super::{fixture, id, Fixture};
use crate::ops::users::run;

/// `who` enrolls and a Box of theirs asks a question.
fn asks(f: &Fixture, who: &Identity, name: &str) {
    let store = f.cx.store().unwrap();
    let (user, _) = store.enroll(who, None).unwrap();
    let new = NewBox {
        name: BoxName::parse(name).unwrap(),
        repo: None,
        spec: SpecFields::default(),
    };
    let boxed = store
        .apply(Decision::insert(new).requester(user))
        .unwrap()
        .box_id;
    let asked = ExtraEvent::NeedsInput {
        question: None,
        options: vec![],
    };
    store.apply(Decision::new(boxed).event(asked)).unwrap();
}

fn inbox(f: &Fixture, who: &Identity, op: InboxOp) -> InboxValue {
    let value = run(&f.cx, &f.vault, who, &UserCmd::Inbox(op)).unwrap();
    serde_json::from_value(value).unwrap()
}

#[test]
fn clearing_another_persons_item_clears_nothing_of_theirs() {
    let f = fixture();
    let (alice, bob) = (id("local:2000"), id("local:2001"));
    asks(&f, &alice, "web/alice-1");
    asks(&f, &bob, "web/bob-1");
    let theirs = inbox(&f, &bob, InboxOp::List).items[0].id;
    let mine = inbox(&f, &alice, InboxOp::Clear(InboxClear::Ids(vec![theirs])));
    assert_eq!(mine.items.len(), 1);
    let mine = inbox(&f, &alice, InboxOp::Clear(InboxClear::Through(i64::MAX)));
    assert!(mine.items.is_empty());
    assert_eq!(mine.unread, 0);
    let after = inbox(&f, &bob, InboxOp::List);
    assert_eq!(
        after.items.iter().map(|i| i.id).collect::<Vec<_>>(),
        [theirs]
    );
    assert_eq!(after.unread, 1);
}
