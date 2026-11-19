//! Tests for `machine-pair`.
#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use std::sync::Arc;

use moor_core::request::{MachinePairReq, PairOp, PairOwner};
use moor_core::user::{Caller, Email, Identity};
use serde_json::{json, Value};

use super::{permit, run};
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;

#[test]
fn only_a_front_may_pair_machines() {
    assert!(permit(&Ok(Caller::Front("gate".to_owned()))).is_ok());
    assert!(permit(&Ok(Caller::Runner)).is_err());
    assert!(permit(&Err("unknown".to_owned())).is_err());
}

fn owner(who: &str) -> PairOwner {
    PairOwner {
        identity: Identity::parse(&format!("authgear:{who}")).unwrap(),
        email: Email::parse(&format!("{who}@example.com")).unwrap(),
    }
}

/// A context over a fresh store.
fn world() -> (tempfile::TempDir, Cx) {
    let dir = tempfile::tempdir().unwrap();
    let cx = Cx::fake(
        dir.path(),
        Arc::new(FakeDriver::new()),
        Arc::new(ScriptedProc::new()),
    );
    (dir, cx)
}

fn step(cx: &Cx, op: PairOp) -> Value {
    run(cx.store().unwrap(), &MachinePairReq { op }).unwrap()
}

fn grant(who: &str, token: &str) -> PairOp {
    PairOp::Grant {
        owner: owner(who),
        token: token.to_owned(),
        ttl_secs: 600,
    }
}

fn redeem(token: &str) -> PairOp {
    PairOp::Redeem {
        token: token.to_owned(),
    }
}

#[test]
fn a_granted_token_names_its_owner_once() {
    let (_dir, cx) = world();
    assert_eq!(
        step(&cx, grant("alice", "tok")),
        json!({"pairing": "started"})
    );
    assert_eq!(
        step(&cx, redeem("tok")),
        json!({"pairing": "granted",
            "owner": {"identity": "authgear:alice", "email": "alice@example.com"}})
    );
    assert_eq!(step(&cx, redeem("tok")), json!({"pairing": "gone"}));
}

#[test]
fn an_unknown_token_is_gone_and_a_new_grant_voids_the_old_token() {
    let (_dir, cx) = world();
    assert_eq!(step(&cx, redeem("nope")), json!({"pairing": "gone"}));
    step(&cx, grant("alice", "one"));
    step(&cx, grant("alice", "two"));
    assert_eq!(step(&cx, redeem("one")), json!({"pairing": "gone"}));
    assert_eq!(step(&cx, redeem("two"))["pairing"], "granted");
}
