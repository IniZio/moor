use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::request::UserCmd;
use moor_core::seam::clock::Clock as _;
use serde_json::{json, Value};

use super::{add, finish, query, secret, setup, signed_in_start, user, Fake};
use crate::ops::users::mcp::for_box;
use crate::ops::users::mcp::oauth::refresh::{refresh_due, Retries};
use crate::ops::users::run;
use crate::ops::users::tests::id;
use crate::testing::clock::FakeClock;

const T0: i64 = 1_800_000_000_000;

/// Signs `local:2000` in to the fake: access token `at-1`, refresh token `rt-1`, an hour long.
fn signed_in(fake: &Fake) -> (super::Fixture, std::sync::Arc<FakeClock>) {
    let (f, clock) = setup(UserConfig::default());
    let q = signed_in_start(&f, fake, "local:2000");
    finish(&f, "local:2000", &q["state"], "good", Some(&fake.base)).unwrap();
    (f, clock)
}

fn refreshes(fake: &Fake) -> Vec<String> {
    fake.hits("POST", "/token")
        .into_iter()
        .filter(|b| b.contains("grant_type=refresh_token"))
        .collect()
}

fn listed(f: &super::Fixture) -> Value {
    run(&f.cx, &f.vault, &id("local:2000"), &UserCmd::McpList).unwrap()["servers"][0].clone()
}

#[test]
fn a_token_near_expiry_is_refreshed_and_the_proxy_secret_changes() {
    let fake = Fake::start(true);
    let (f, clock) = signed_in(&fake);
    let me = user(&f, "local:2000");
    let mut retries = Retries::default();
    clock.sleep(Duration::from_mins(50));
    assert!(refresh_due(&f.cx, &f.vault, &mut retries).is_empty());
    assert!(
        refreshes(&fake).is_empty(),
        "ten minutes left of an hour is not due"
    );
    clock.sleep(Duration::from_secs(250));
    assert!(refresh_due(&f.cx, &f.vault, &mut retries).is_empty());
    assert_eq!(
        secret(&f, &f.vault, me).as_deref(),
        Some("at-2"),
        "350s left is inside a tenth of the hour"
    );
    let form = query(&format!("http://x/?{}", refreshes(&fake)[0]));
    assert_eq!(form["refresh_token"], "rt-1");
    assert_eq!(form["resource"], fake.mcp());
    assert_eq!(listed(&f)["signed_in"], json!(T0 + 3_250_000 + 3_600_000));
}

#[test]
fn a_rotated_refresh_token_is_used_next_time_and_a_kept_one_stays() {
    let fake = Fake::start(true);
    let (f, clock) = signed_in(&fake);
    let mut retries = Retries::default();
    clock.sleep(Duration::from_secs(3500));
    refresh_due(&f.cx, &f.vault, &mut retries);
    *fake.knobs.refresh.lock().unwrap() = "keep";
    clock.sleep(Duration::from_secs(3500));
    refresh_due(&f.cx, &f.vault, &mut retries);
    clock.sleep(Duration::from_secs(3500));
    refresh_due(&f.cx, &f.vault, &mut retries);
    let sent: Vec<String> = refreshes(&fake)
        .iter()
        .map(|b| query(&format!("http://x/?{b}"))["refresh_token"].clone())
        .collect();
    assert_eq!(sent, ["rt-1", "rt-2", "rt-2"]);
}

#[test]
fn a_5xx_keeps_the_old_token_and_retries_after_a_backoff() {
    let fake = Fake::start(true);
    let (f, clock) = signed_in(&fake);
    let me = user(&f, "local:2000");
    *fake.knobs.refresh.lock().unwrap() = "5xx";
    let mut retries = Retries::default();
    clock.sleep(Duration::from_secs(3500));
    let msgs = refresh_due(&f.cx, &f.vault, &mut retries);
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(msgs[0].contains("retrying in 30s"), "{msgs:?}");
    assert_eq!(secret(&f, &f.vault, me).as_deref(), Some("at-1"));
    refresh_due(&f.cx, &f.vault, &mut retries);
    assert_eq!(
        refreshes(&fake).len(),
        1,
        "no retry before the backoff ends"
    );
    *fake.knobs.refresh.lock().unwrap() = "rotate";
    clock.sleep(Duration::from_secs(30));
    assert!(refresh_due(&f.cx, &f.vault, &mut retries).is_empty());
    assert_eq!(refreshes(&fake).len(), 2);
    assert_eq!(secret(&f, &f.vault, me).as_deref(), Some("at-2"));
}

#[test]
fn invalid_grant_clears_the_secret_and_asks_to_sign_in_again() {
    let fake = Fake::start(true);
    let (f, clock) = signed_in(&fake);
    let me = user(&f, "local:2000");
    *fake.knobs.refresh.lock().unwrap() = "invalid";
    let mut retries = Retries::default();
    clock.sleep(Duration::from_secs(3500));
    let msgs = refresh_due(&f.cx, &f.vault, &mut retries);
    assert_eq!(
        msgs,
        [format!(
            "oauth: user {} MCP server notion needs sign-in",
            me.get()
        )]
    );
    assert_eq!(secret(&f, &f.vault, me), None);
    let row = listed(&f);
    assert_eq!(row["sign_in_again"], true);
    assert_eq!(row.get("signed_in"), None);
    let boxed = for_box(&f.cx, Some(me)).unwrap();
    assert_eq!(
        boxed.notes,
        ["MCP server notion left out: its sign-in ended; sign in again with mcp-add notion"]
    );
    clock.sleep(Duration::from_secs(3500));
    assert!(refresh_due(&f.cx, &f.vault, &mut retries).is_empty());
    assert_eq!(
        refreshes(&fake).len(),
        1,
        "a signed-out server is left alone"
    );

    let again = add(&f, "local:2000", &fake.mcp()).unwrap();
    let q = query(again["sign_in"].as_str().unwrap());
    finish(&f, "local:2000", &q["state"], "good", Some(&fake.base)).unwrap();
    let row = listed(&f);
    assert_eq!(row.get("sign_in_again"), None);
    assert_eq!(secret(&f, &f.vault, me).as_deref(), Some("at-1"));
}
