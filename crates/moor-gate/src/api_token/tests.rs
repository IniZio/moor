#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    reason = "tests pick their own instants"
)]

use std::time::{Duration, Instant};

use moor_core::user::{Email, Identity};

use super::{Cache, MAX, TTL};
use crate::Person;

fn person(n: usize) -> Person {
    Person {
        identity: Identity::parse(&format!("authgear:sub-{n}")).unwrap(),
        email: Email::parse(&format!("p{n}@example.com")).unwrap(),
    }
}

fn key(n: usize) -> [u8; 32] {
    let mut key = [0; 32];
    key[..8].copy_from_slice(&n.to_le_bytes());
    key
}

#[test]
fn an_answer_is_trusted_for_thirty_seconds_only() {
    let cache = Cache::default();
    let start = Instant::now();
    cache.keep(key(1), person(1), vec!["moor:read".to_owned()], start);
    let soon = cache.lookup(&key(1), start + Duration::from_secs(29));
    assert_eq!(soon.map(|(p, _)| p), Some(person(1)));
    assert!(cache.lookup(&key(1), start + TTL).is_none());
}

#[test]
fn the_oldest_answer_goes_first_past_the_cap() {
    let cache = Cache::default();
    let start = Instant::now();
    for n in 0..=MAX {
        let at = start + Duration::from_millis(u64::try_from(n).unwrap());
        cache.keep(key(n), person(n), vec![], at);
    }
    let at = start + Duration::from_secs(1);
    assert!(cache.lookup(&key(0), at).is_none());
    assert!(cache.lookup(&key(MAX), at).is_some());
}
