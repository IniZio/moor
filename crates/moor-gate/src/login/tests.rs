#![allow(
    clippy::unwrap_used,
    clippy::disallowed_methods,
    reason = "tests pick their own instants"
)]

use std::time::{Duration, SystemTime};

use moor_core::user::{Email, Identity};

use super::{
    check_csrf, csrf_token, Login, Pending, PENDING_PURPOSE, PENDING_TTL, SESSION_PURPOSE,
    SESSION_TTL,
};
use crate::Person;

const KEY: &str = "0123456789abcdef0123456789abcdef-one";
const OTHER: &str = "0123456789abcdef0123456789abcdef-two";

fn login() -> Login {
    Login::with_keys("https://moor.example.com", KEY).unwrap()
}

fn pending() -> Pending {
    Pending {
        state: "s".to_owned(),
        verifier: "v".to_owned(),
        back: "/auth/me".to_owned(),
        token_endpoint: "https://issuer.example/token".to_owned(),
    }
}

fn alice() -> Person {
    Person {
        identity: Identity::parse("authgear:alice-sub").unwrap(),
        email: Email::parse("alice@example.com").unwrap(),
    }
}

fn sealed_pending(l: &Login, now: SystemTime) -> String {
    let sealed = l.seal.seal(PENDING_PURPOSE, &pending(), now, PENDING_TTL);
    format!("moor_login={}", sealed.unwrap())
}

#[test]
fn the_redirect_uri_is_the_public_url_callback() {
    assert_eq!(login().redirect, "https://moor.example.com/auth/callback");
    let trailing = Login::new("https://moor.example.com/").unwrap();
    assert_eq!(trailing.redirect, "https://moor.example.com/auth/callback");
    assert!(Login::new("moor.example.com").is_err());
}

#[test]
fn a_key_must_be_long_and_present() {
    let url = "https://moor.example.com";
    assert!(Login::with_keys(url, "short").is_err());
    assert!(Login::with_keys(url, " , ").is_err());
    assert!(Login::with_keys(url, &format!("{KEY}, {OTHER}")).is_ok());
}

#[test]
fn a_sign_in_opens_on_another_login_with_the_same_key() {
    let t0 = SystemTime::now();
    let cookie = sealed_pending(&login(), t0);
    let elsewhere = login();
    assert_eq!(
        elsewhere.take([cookie.as_str()], "s", t0).unwrap().back,
        "/auth/me"
    );
    let stranger = Login::with_keys("https://moor.example.com", OTHER).unwrap();
    assert!(stranger.take([cookie.as_str()], "s", t0).is_none());
}

#[test]
fn a_sign_in_is_refused_for_another_state() {
    let (l, t0) = (login(), SystemTime::now());
    let cookie = sealed_pending(&l, t0);
    assert!(l.take([cookie.as_str()], "t", t0).is_none());
    assert!(l.take([cookie.as_str()], "", t0).is_none());
    assert!(l.take([], "s", t0).is_none());
}

#[test]
fn a_sign_in_expires_after_ten_minutes() {
    let (l, t0) = (login(), SystemTime::now());
    let cookie = sealed_pending(&l, t0);
    let at = |secs| t0 + Duration::from_secs(secs);
    assert!(l.take([cookie.as_str()], "s", at(599)).is_some());
    assert!(l.take([cookie.as_str()], "s", t0 + PENDING_TTL).is_none());
}

#[test]
fn a_tampered_sign_in_is_refused() {
    let (l, t0) = (login(), SystemTime::now());
    let mut cookie = sealed_pending(&l, t0);
    let last = cookie.pop().unwrap();
    cookie.push(if last == 'A' { 'B' } else { 'A' });
    let forged = cookie;
    assert!(l.take([forged.as_str()], "s", t0).is_none());
    assert!(l.take(["moor_login=forged"], "s", t0).is_none());
}

#[test]
fn a_session_lives_fifteen_minutes_from_sign_in() {
    let (l, t0) = (login(), SystemTime::now());
    let cookie = format!("theme=dark; moor_session={}", l.open(alice(), t0).unwrap());
    let later = t0 + Duration::from_secs(899);
    assert_eq!(l.session([cookie.as_str()], later).unwrap().person, alice());
    assert!(l.session([cookie.as_str()], t0 + SESSION_TTL).is_none());
    assert!(l.session(["moor_session=forged"], t0).is_none());
}

#[test]
fn a_cookie_opens_only_for_its_own_purpose() {
    let (l, t0) = (login(), SystemTime::now());
    let sign_in = l.seal.seal(PENDING_PURPOSE, &pending(), t0, SESSION_TTL);
    let as_session = format!("moor_session={}", sign_in.unwrap());
    assert!(l.session([as_session.as_str()], t0).is_none());
    let session = l.open(alice(), t0).unwrap();
    let open: Option<Pending> = l.seal.open(SESSION_PURPOSE, &session, t0);
    assert!(open.is_none());
}

#[test]
fn an_older_key_still_opens_what_it_sealed() {
    let (old, t0) = (login(), SystemTime::now());
    let cookie = format!("moor_session={}", old.open(alice(), t0).unwrap());
    let rotated = Login::with_keys("https://moor.example.com", &format!("{OTHER},{KEY}")).unwrap();
    assert!(rotated.session([cookie.as_str()], t0).is_some());
    let dropped = Login::with_keys("https://moor.example.com", OTHER).unwrap();
    assert!(dropped.session([cookie.as_str()], t0).is_none());
}

#[test]
fn the_csrf_token_belongs_to_its_session() {
    let (l, t0) = (login(), SystemTime::now());
    let open = |person| {
        let cookie = format!("moor_session={}", l.open(person, t0).unwrap());
        l.session([cookie.as_str()], t0).unwrap()
    };
    let (a, b) = (open(alice()), open(alice()));
    assert!(csrf_token(&a).len() >= 43);
    assert!(check_csrf(&a, csrf_token(&a)));
    assert!(!check_csrf(&a, csrf_token(&b)));
    assert!(!check_csrf(&a, ""));
}
