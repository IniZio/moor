#![allow(
    clippy::unwrap_used,
    clippy::disallowed_methods,
    reason = "tests pick their own instants"
)]

use std::time::{Duration, Instant};

use moor_core::user::{Email, Identity};

use super::{check_csrf, csrf_token, Login, Pending, PENDING_MAX, PENDING_TTL, SESSION_TTL};
use crate::Person;

fn login() -> Login {
    Login::new("https://moor.example.com").unwrap()
}

fn pending(created: Instant) -> Pending {
    Pending {
        verifier: "v".to_owned(),
        back: "/auth/me".to_owned(),
        token_endpoint: "https://issuer.example/token".to_owned(),
        created,
    }
}

fn alice() -> Person {
    Person {
        identity: Identity::parse("authgear:alice-sub").unwrap(),
        email: Email::parse("alice@example.com").unwrap(),
    }
}

#[test]
fn the_redirect_uri_is_the_public_url_callback() {
    assert_eq!(login().redirect, "https://moor.example.com/auth/callback");
    let trailing = Login::new("https://moor.example.com/").unwrap();
    assert_eq!(trailing.redirect, "https://moor.example.com/auth/callback");
    assert!(Login::new("moor.example.com").is_err());
}

#[test]
fn a_state_is_taken_once() {
    let (l, t0) = (login(), Instant::now());
    l.remember("s".to_owned(), pending(t0), t0);
    assert_eq!(l.take("s", t0).unwrap().back, "/auth/me");
    assert!(l.take("s", t0).is_none());
}

#[test]
fn a_state_expires_after_ten_minutes() {
    let (l, t0) = (login(), Instant::now());
    l.remember("s".to_owned(), pending(t0), t0);
    assert!(l.take("s", t0 + PENDING_TTL).is_none());
    l.remember("t".to_owned(), pending(t0), t0);
    assert!(l.take("t", t0 + Duration::from_secs(599)).is_some());
}

#[test]
fn waiting_sign_ins_are_capped_dropping_the_oldest() {
    let (l, t0) = (login(), Instant::now());
    for i in 0..=PENDING_MAX {
        let at = t0 + Duration::from_millis(u64::try_from(i).unwrap());
        l.remember(format!("s{i}"), pending(at), at);
    }
    assert_eq!(l.pending.lock().unwrap().len(), PENDING_MAX);
    assert!(l.take("s0", t0).is_none());
    assert!(l.take(&format!("s{PENDING_MAX}"), t0).is_some());
}

#[test]
fn a_session_lives_fifteen_minutes_from_sign_in() {
    let (l, t0) = (login(), Instant::now());
    let s = l.open(alice(), t0);
    let cookie = format!("theme=dark; moor_session={}", s.id);
    let later = t0 + Duration::from_secs(899);
    assert_eq!(l.session([cookie.as_str()], later).unwrap().person, alice());
    assert!(l.session([cookie.as_str()], t0 + SESSION_TTL).is_none());
    assert!(l.session(["moor_session=forged"], t0).is_none());
    l.close(&s);
    assert!(l.session([cookie.as_str()], t0).is_none());
}

#[test]
fn the_csrf_token_belongs_to_its_session() {
    let (l, t0) = (login(), Instant::now());
    let (a, b) = (l.open(alice(), t0), l.open(alice(), t0));
    assert!(csrf_token(&a).len() >= 43);
    assert!(check_csrf(&a, csrf_token(&a)));
    assert!(!check_csrf(&a, csrf_token(&b)));
    assert!(!check_csrf(&a, ""));
    assert_ne!(a.id, b.id);
}
