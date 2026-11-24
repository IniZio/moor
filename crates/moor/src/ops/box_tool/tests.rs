//! A claimed place's Box tools reach its own user's work Boxes and nothing else.

use std::collections::BTreeSet;

use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::domain::place::Claim;
use moor_core::request::{BoxCall, BoxExecSecs, BoxToolReq, Grant, Pins};
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proc::Pipes;
use moor_core::user::{Caller, Identity, Viewer};
use moor_core::{BoxId, BoxName, BoxTarget, ErrorKind};

use super::{admit, Admitted, Plan};
use crate::ops::run_fixture::{ready_with, Fx};
use crate::testing::cx::{new_idle_place, requester, REQUESTER_USER};
use crate::testing::driver::FakeDriver;
use crate::Moor;

/// The gate's front, which runs the Box tools.
fn gate() -> Caller {
    Caller::Front("gate".to_owned())
}

fn name(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).unwrap()
}

/// Ready work Box `t` on `o/n` with a claimed place `p` and an idle place `idle` beside it,
/// all the requester's, and work Box `theirs` of another user; the gate front runs the tools.
struct World {
    f: Fx,
    place: BoxId,
    idle: BoxId,
}

fn world() -> World {
    let mut f = ready_with(FakeDriver::new());
    f.cx =
        f.cx.with_env(|e| e.with_front_box_tools(BTreeSet::from(["gate".to_owned()])));
    let store = f.cx.store().unwrap();
    let place = store.apply(new_idle_place(name("p"))).unwrap().box_id;
    store
        .put_claim(
            place,
            REQUESTER_USER,
            &Claim {
                repo: ForgeRepo::new("o", "n"),
                also: vec![ForgeRepo::new("o", "m")],
                prompt: "where is it".to_owned(),
                at: 1,
                served: Some(2),
                steered: Some(3),
            },
        )
        .unwrap();
    let idle = store.apply(new_idle_place(name("idle"))).unwrap().box_id;
    let other = Identity::parse("gate:other").unwrap();
    let (them, _) = store.enroll(&other, None).unwrap();
    store
        .apply(
            Decision::insert(NewBox {
                name: name("theirs"),
                repo: None,
                spec: SpecFields::default(),
            })
            .requester(them),
        )
        .unwrap();
    World { f, place, idle }
}

/// `ls` in `target`, from Box `from`.
fn exec(from: BoxId, target: &str) -> BoxToolReq {
    BoxToolReq {
        from,
        call: BoxCall::Exec {
            target: name(target),
            command: "echo hi from $PWD".to_owned(),
            timeout: BoxExecSecs::new(None).unwrap(),
        },
    }
}

/// A start of `n2` on `repo` with `also` beside it, from Box `from`.
fn start(from: BoxId, repo: &str, also: &[&str]) -> BoxToolReq {
    let parse = |r: &str| ForgeRepo::parse(r).unwrap();
    BoxToolReq {
        from,
        call: BoxCall::Start {
            name: name("n2"),
            repo: parse(repo),
            also: also.iter().map(|r| parse(r)).collect(),
            prompt: Some("go".to_owned()),
        },
    }
}

impl World {
    fn admit(&self, req: &BoxToolReq) -> Result<Admitted, String> {
        self.admit_as(&Ok(gate()), None, req)
    }

    fn admit_as(
        &self,
        caller: &Result<Caller, String>,
        asserted: Option<&Identity>,
        req: &BoxToolReq,
    ) -> Result<Admitted, String> {
        admit(&self.f.cx, caller, asserted, req).map_err(|e| {
            assert_eq!(e.kind, ErrorKind::Refused, "{}", e.error);
            e.error.to_string()
        })
    }
}

#[track_caller]
fn refused_with(got: Result<Admitted, String>, want: &str) {
    let why = got.map(|admitted| format!("{admitted:?}")).unwrap_err();
    assert!(why.contains(want), "{why}");
}

#[test]
fn a_box_tool_from_a_claimed_place_execs_in_its_user_s_work_box() {
    let w = world();
    let admitted = w.admit(&exec(w.place, "t")).unwrap();
    assert_eq!(admitted.user, REQUESTER_USER);
    assert_eq!(admitted.who, requester());
    let req = match admitted.plan {
        Plan::Exec(req) => Some(req),
        Plan::Start(_) => None,
    }
    .unwrap();
    assert_eq!(req.target, BoxTarget::Id(w.f.id));
    assert_eq!(req.timeout_secs, Some(30));
    let moor = Moor::from_cx(w.f.cx).granted(Grant {
        viewer: Viewer::User(admitted.user),
        pins: Pins::new(),
    });
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let view = moor
        .exec(
            &req,
            Pipes {
                stdin: Box::new(std::io::empty()),
                stdout: &mut out,
                stderr: &mut err,
                stop: &|| false,
            },
        )
        .unwrap();
    assert_eq!(view.exit, Some(0));
    assert!(
        String::from_utf8_lossy(&out).starts_with("hi from "),
        "{out:?}"
    );
}

#[test]
fn a_box_tool_from_a_work_box_is_refused() {
    let w = world();
    refused_with(w.admit(&exec(w.f.id, "t")), "this is a work Box");
    refused_with(w.admit(&start(w.f.id, "o/n", &[])), "this is a work Box");
}

#[test]
fn a_box_tool_from_an_idle_place_is_refused() {
    let w = world();
    refused_with(w.admit(&exec(w.idle, "t")), "once a task claims it");
}

#[test]
fn a_box_tool_on_another_user_s_box_is_refused_as_not_yours() {
    let w = world();
    refused_with(w.admit(&exec(w.place, "theirs")), "not your Box");
    refused_with(w.admit(&exec(w.place, "nowhere")), "not your Box");
}

#[test]
fn a_box_tool_on_a_place_is_refused() {
    let w = world();
    refused_with(w.admit(&exec(w.place, "idle")), "only work Boxes");
    refused_with(w.admit(&exec(w.place, "p")), "only work Boxes");
}

#[test]
fn a_box_tool_from_an_unflagged_front_or_any_other_caller_is_refused() {
    let w = world();
    let req = exec(w.place, "t");
    for caller in [
        Ok(Caller::Front("web".to_owned())),
        Ok(Caller::Operator(1000)),
        Ok(Caller::Local(1001)),
        Ok(Caller::Runner),
        Err("no peer".to_owned()),
    ] {
        refused_with(w.admit_as(&caller, None, &req), "`box_tools: true`");
    }
}

#[test]
fn a_box_tool_that_asserts_an_identity_is_refused() {
    let w = world();
    let person = Identity::parse("gate:other").unwrap();
    refused_with(
        w.admit_as(&Ok(gate()), Some(&person), &exec(w.place, "t")),
        "asserts no identity",
    );
}

#[test]
fn start_from_a_place_may_name_only_its_claim_repos() {
    let w = world();
    let admitted = w.admit(&start(w.place, "o/m", &["o/n"])).unwrap();
    let req = match admitted.plan {
        Plan::Start(req) => Some(req),
        Plan::Exec(_) => None,
    }
    .unwrap();
    assert_eq!(req.repo, Some(ForgeRepo::new("o", "m")));
    assert_eq!(req.also, Some(vec![ForgeRepo::new("o", "n")]));
    assert_eq!(req.prompt.as_deref(), Some("go"));
    assert!(!req.place && req.config.is_none() && req.identity.is_none());
    let only_repo = w.admit(&start(w.place, "o/n", &[])).unwrap();
    assert!(matches!(only_repo.plan, Plan::Start(ref r) if r.also == Some(vec![])));
    refused_with(
        w.admit(&start(w.place, "o/x", &[])),
        "only this task's repos (o/n, o/m), not o/x",
    );
    refused_with(w.admit(&start(w.place, "o/n", &["o/x"])), "not o/x");
}
