#![allow(
    clippy::unwrap_used,
    clippy::disallowed_methods,
    reason = "tests pick their own instants"
)]

use std::time::{Duration, SystemTime};

use moor_core::user::{Email, Identity};

use super::{
    check_csrf, csrf_token, kept, Login, Pending, PENDING_PURPOSE, PENDING_TTL, SESSION_IDLE,
    SESSION_MAX, SESSION_PURPOSE,
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
fn a_session_lasts_twelve_hours_from_its_last_renewal() {
    let (l, t0) = (login(), SystemTime::now());
    let at = |secs| t0 + Duration::from_secs(secs);
    let cookie = |sealed: String| format!("theme=dark; moor_session={sealed}");
    let first = cookie(l.open(alice(), t0).unwrap().sealed);
    let session = l.session([first.as_str()], at(43_199)).unwrap();
    assert_eq!(session.person, alice());
    assert!(l.session([first.as_str()], t0 + SESSION_IDLE).is_none());
    let renewed = l.renew(&session, at(43_000)).unwrap();
    assert_eq!(renewed.ttl, SESSION_IDLE);
    let second = cookie(renewed.sealed);
    let later = l.session([second.as_str()], at(43_000 + 43_199)).unwrap();
    assert_eq!(
        csrf_token(&later),
        csrf_token(&session),
        "a renewal keeps the token"
    );
    assert!(l.session(["moor_session=forged"], t0).is_none());
}

#[test]
fn renewals_never_carry_a_session_past_a_day_from_sign_in() {
    let (l, t0) = (login(), SystemTime::now());
    let first = format!("moor_session={}", l.open(alice(), t0).unwrap().sealed);
    let session = l.session([first.as_str()], t0).unwrap();
    assert_eq!(SESSION_MAX, Duration::from_hours(24));
    let late = l.renew(&session, t0 + Duration::from_hours(20)).unwrap();
    assert_eq!(
        late.ttl,
        Duration::from_hours(4),
        "the cookie's Max-Age shrinks to the cap"
    );
    assert!(
        late.header().contains("Max-Age=14400;"),
        "{}",
        late.header()
    );
    let cookie = format!("moor_session={}", late.sealed);
    let just_before = t0 + SESSION_MAX - Duration::from_secs(1);
    assert!(l.session([cookie.as_str()], just_before).is_some());
    assert!(l.session([cookie.as_str()], t0 + SESSION_MAX).is_none());
    assert!(l.renew(&session, t0 + SESSION_MAX).is_err());
}

#[test]
fn a_kept_key_is_made_once_and_read_after() {
    let dir = std::env::temp_dir().join(format!("moor-gate-kept-{}", std::process::id()));
    let path = dir.join("moor-gate/cookie-key");
    let made = kept::read_or_make(&path).unwrap();
    assert!(made.len() >= 64, "{made}");
    assert_eq!(kept::read_or_make(&path).unwrap(), made);
    let perms =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions());
    assert_eq!(perms & 0o777, 0o600);
    let t0 = SystemTime::now();
    let before = Login::with_kept_key("https://moor.example.com", &path).unwrap();
    let cookie = format!("moor_session={}", before.open(alice(), t0).unwrap().sealed);
    let restarted = Login::with_kept_key("https://moor.example.com", &path).unwrap();
    assert!(restarted.session([cookie.as_str()], t0).is_some());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_cookie_opens_only_for_its_own_purpose() {
    let (l, t0) = (login(), SystemTime::now());
    let sign_in = l.seal.seal(PENDING_PURPOSE, &pending(), t0, SESSION_IDLE);
    let as_session = format!("moor_session={}", sign_in.unwrap());
    assert!(l.session([as_session.as_str()], t0).is_none());
    let session = l.open(alice(), t0).unwrap().sealed;
    let open: Option<Pending> = l.seal.open(SESSION_PURPOSE, &session, t0);
    assert!(open.is_none());
}

#[test]
fn an_older_key_still_opens_what_it_sealed() {
    let (old, t0) = (login(), SystemTime::now());
    let cookie = format!("moor_session={}", old.open(alice(), t0).unwrap().sealed);
    let rotated = Login::with_keys("https://moor.example.com", &format!("{OTHER},{KEY}")).unwrap();
    assert!(rotated.session([cookie.as_str()], t0).is_some());
    let dropped = Login::with_keys("https://moor.example.com", OTHER).unwrap();
    assert!(dropped.session([cookie.as_str()], t0).is_none());
}

#[test]
fn the_csrf_token_belongs_to_its_session() {
    let (l, t0) = (login(), SystemTime::now());
    let open = |person| {
        let cookie = format!("moor_session={}", l.open(person, t0).unwrap().sealed);
        l.session([cookie.as_str()], t0).unwrap()
    };
    let (a, b) = (open(alice()), open(alice()));
    assert!(csrf_token(&a).len() >= 43);
    assert!(check_csrf(&a, csrf_token(&a)));
    assert!(!check_csrf(&a, csrf_token(&b)));
    assert!(!check_csrf(&a, ""));
}
