//! A claimed place's Box tools reach its own user's work Boxes and nothing else.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::request::{BoxCall, BoxExecSecs, BoxRunId, BoxToolReq, Grant, Pins};
use moor_core::seam::driver::BoxStep;
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proc::Pipes;
use moor_core::user::{Caller, Identity, Viewer};
use moor_core::view::{BoxRunState, BoxRunView};
use moor_core::{BoxId, BoxName, BoxTarget, CloseReq, ErrorKind, ExitClass, Request};

use super::{admit, for_place, runs, Admitted, Plan};
use crate::cx::Cx;
use crate::ops::common::boxwork;
use crate::ops::run_fixture::{ready_with, Fx};
use crate::testing::cx::{new_idle_place, requester, REQUESTER_USER};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::Moor;

mod long_runs;

/// The gate's front, which runs the Box tools.
fn gate() -> Caller {
    Caller::Front("gate".to_owned())
}

fn name(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).unwrap()
}

/// Ready work Box `t` on `o/n` with a claimed place `p` and an idle place `idle` beside it,
/// all the requester's, and work Box `theirs` and claimed place `q` of another user; the gate
/// front runs the tools.
struct World {
    f: Fx,
    place: BoxId,
    idle: BoxId,
    /// The other user's claimed place.
    their_place: BoxId,
    /// The requester's claimed place beside `place`, on another task.
    other_place: BoxId,
}

/// The claim of a place on `o/n` and `o/m`.
fn claim() -> Claim {
    Claim {
        repo: ForgeRepo::new("o", "n"),
        also: vec![ForgeRepo::new("o", "m")],
        prompt: "where is it".to_owned(),
        at: 1,
        served: Some(2),
        steered: Some(3),
    }
}

fn world() -> World {
    let mut f = ready_with(FakeDriver::new());
    f.cx =
        f.cx.with_env(|e| e.with_front_box_tools(BTreeSet::from(["gate".to_owned()])));
    let store = f.cx.store().unwrap();
    let place = store.apply(new_idle_place(name("p"))).unwrap().box_id;
    store.put_claim(place, REQUESTER_USER, &claim()).unwrap();
    store.tie_to_task(f.id, place).unwrap();
    let other_place = store.apply(new_idle_place(name("p2"))).unwrap().box_id;
    store
        .put_claim(other_place, REQUESTER_USER, &claim())
        .unwrap();
    work_box_of(store, "elsewhere", Some(other_place));
    work_box_of(store, "plain", None);
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
    let their_place = store
        .apply(
            Decision::insert(NewBox {
                name: name("q"),
                repo: None,
                spec: SpecFields {
                    kind: BoxKind::Place(PlaceState::Idle),
                    ..SpecFields::default()
                },
            })
            .requester(them),
        )
        .unwrap()
        .box_id;
    store.put_claim(their_place, them, &claim()).unwrap();
    World {
        f,
        place,
        idle,
        their_place,
        other_place,
    }
}

/// A work Box `n` of the requester, started by place `task` or by no place.
fn work_box_of(store: &crate::store::Store, n: &str, task: Option<BoxId>) {
    store
        .apply(
            Decision::insert(NewBox {
                name: name(n),
                repo: None,
                spec: SpecFields {
                    task,
                    ..SpecFields::default()
                },
            })
            .requester(REQUESTER_USER),
        )
        .unwrap();
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
            no_setup: false,
        },
    }
}

/// A close of `target`, from Box `from`.
fn close(from: BoxId, target: &str) -> BoxToolReq {
    BoxToolReq {
        from,
        call: BoxCall::Close {
            target: name(target),
        },
    }
}

impl World {
    /// What the work Box's probe reads at close: `text`, as `moor close` would find it.
    fn probe(&self, text: &str) {
        let store = self.f.cx.store().unwrap();
        let row = store.box_row(self.f.id).unwrap().unwrap();
        let hands = row.hands.as_ref().unwrap();
        let plan = boxwork::plan(&self.f.cx, &row, hands).ok().unwrap();
        let handle = hands.handle.as_ref().unwrap();
        for step in &plan.steps {
            if let BoxStep::Peek(path) = step {
                self.f.driver.set_peek_text(handle.as_str(), path, text);
            }
        }
    }

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
        Plan::Start(_) | Plan::Close(_) | Plan::Wait(_) => None,
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

/// Each Box `from` may not reach is refused alike, naming the Box.
#[track_caller]
fn refused_as_not_this_task(w: &World, from: BoxId, call: fn(BoxId, &str) -> BoxToolReq) {
    for target in ["elsewhere", "plain", "theirs", "nowhere"] {
        let want = format!("not your Box: Box {target} is not a work Box this task started");
        refused_with(w.admit(&call(from, target)), &want);
    }
}

#[test]
fn exec_reaches_only_a_work_box_the_place_started() {
    let w = world();
    assert!(w.admit(&exec(w.place, "t")).is_ok());
    assert!(w.admit(&exec(w.other_place, "elsewhere")).is_ok());
    refused_as_not_this_task(&w, w.place, exec);
}

#[test]
fn close_reaches_only_a_work_box_the_place_started() {
    let w = world();
    assert!(w.admit(&close(w.place, "t")).is_ok());
    assert!(w.admit(&close(w.other_place, "elsewhere")).is_ok());
    refused_as_not_this_task(&w, w.place, close);
}

#[test]
fn a_closed_box_of_another_task_is_not_named_as_closed() {
    let w = world();
    w.probe("ahead=0 dirty=0");
    let req = close_req(w.admit(&close(w.place, "t")).unwrap());
    crate::ops::close::requested(&w.f.cx, &req).unwrap();
    refused_with(
        w.admit(&close(w.other_place, "t")),
        "belongs to another task or none",
    );
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
fn a_start_from_a_place_ties_the_box_to_the_task_and_starts_no_agent() {
    let w = world();
    let admitted = w.admit(&start(w.place, "o/n", &[])).unwrap();
    let req = match admitted.plan {
        Plan::Start(req) => Some(req),
        Plan::Exec(_) | Plan::Close(_) | Plan::Wait(_) => None,
    }
    .unwrap();
    assert_eq!(req.task, Some(w.place));
    assert!(req.no_agent && req.prompt.is_none());
    assert_eq!(req.name, name("n2"));
}

#[test]
fn start_from_a_place_may_name_only_its_claim_repos() {
    let w = world();
    let admitted = w.admit(&start(w.place, "o/m", &["o/n"])).unwrap();
    let req = match admitted.plan {
        Plan::Start(req) => Some(req),
        Plan::Exec(_) | Plan::Close(_) | Plan::Wait(_) => None,
    }
    .unwrap();
    assert_eq!(req.repo, Some(ForgeRepo::new("o", "m")));
    assert_eq!(req.also, Some(vec![ForgeRepo::new("o", "n")]));
    assert!(req.prompt.is_none() && !req.place && req.config.is_none() && req.identity.is_none());
    let only_repo = w.admit(&start(w.place, "o/n", &[])).unwrap();
    assert!(matches!(only_repo.plan, Plan::Start(ref r) if r.also == Some(vec![])));
    refused_with(
        w.admit(&start(w.place, "o/x", &[])),
        "only this task's repos (o/n, o/m), not o/x",
    );
    refused_with(w.admit(&start(w.place, "o/n", &["o/x"])), "not o/x");
}

/// The close `admitted` runs.
fn close_req(admitted: Admitted) -> CloseReq {
    match admitted.plan {
        Plan::Close(req) => Some(req),
        Plan::Exec(_) | Plan::Start(_) | Plan::Wait(_) => None,
    }
    .unwrap()
}

#[test]
fn a_close_from_the_place_closes_its_user_s_work_box() {
    let w = world();
    w.probe("ahead=0 dirty=0");
    let admitted = w.admit(&close(w.place, "t")).unwrap();
    assert_eq!(admitted.user, REQUESTER_USER);
    let who = admitted.who.clone();
    let grant = Grant {
        viewer: Viewer::User(admitted.user),
        pins: Pins::new(),
    };
    let req = close_req(admitted);
    assert_eq!(req.target, BoxTarget::Id(w.f.id));
    assert!(!req.force);
    let reply = Moor::from_cx(w.f.cx)
        .granted(grant)
        .acting_as(who)
        .call(Request::Close(req));
    assert_eq!(reply.class, ExitClass::Ok, "{:?}", reply.error);
    let value = reply.value.unwrap();
    assert_eq!(
        value.pointer("/box/lifecycle"),
        Some(&serde_json::json!("closed")),
        "{value}"
    );
}

#[test]
fn a_close_of_another_user_s_box_is_refused_as_not_yours() {
    let w = world();
    refused_with(w.admit(&close(w.place, "theirs")), "not your Box");
    refused_with(w.admit(&close(w.place, "nowhere")), "not your Box");
}

#[test]
fn a_close_of_a_place_is_refused() {
    let w = world();
    refused_with(w.admit(&close(w.place, "idle")), "only work Boxes");
    refused_with(w.admit(&close(w.place, "p")), "only work Boxes");
}

#[test]
fn a_close_from_a_work_box_is_refused() {
    let w = world();
    refused_with(w.admit(&close(w.f.id, "t")), "this is a work Box");
}

#[test]
fn a_close_from_an_idle_place_is_refused() {
    let w = world();
    refused_with(w.admit(&close(w.idle, "t")), "once a task claims it");
}

#[test]
fn unpushed_work_is_refused_and_force_is_not_offered() {
    let w = world();
    w.probe("ahead=1 dirty=2");
    let req = close_req(w.admit(&close(w.place, "t")).unwrap());
    assert!(!req.force);
    let e = crate::ops::close::requested(&w.f.cx, &req).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Refused);
    assert!(
        e.error.to_string().contains("1 unpushed commit"),
        "{}",
        e.error
    );
    let row = w.f.cx.store().unwrap().box_row(w.f.id).unwrap().unwrap();
    assert_ne!(row.lifecycle, BoxLifecycle::Closed);
    let wire = r#"{"from":3,"call":{"tool":"close","target":"t","force":true}}"#;
    assert!(serde_json::from_str::<BoxToolReq>(wire).is_err());
}

#[test]
fn a_second_close_names_the_box_as_already_closed() {
    let w = world();
    w.probe("ahead=0 dirty=0");
    let req = close_req(w.admit(&close(w.place, "t")).unwrap());
    crate::ops::close::requested(&w.f.cx, &req).unwrap();
    refused_with(w.admit(&close(w.place, "t")), "Box t is already closed");
    refused_with(w.admit(&close(w.place, "theirs")), "not your Box");
}

#[test]
fn unpushed_work_is_refused_with_what_the_place_can_do() {
    let w = world();
    w.probe("ahead=1 dirty=2");
    let admitted = w.admit(&close(w.place, "t")).unwrap();
    let (user, who) = (admitted.user, admitted.who.clone());
    let reply = Moor::from_cx(w.f.cx)
        .granted(Grant {
            viewer: Viewer::User(user),
            pins: Pins::new(),
        })
        .acting_as(who)
        .call(Request::Close(close_req(admitted)));
    let error = for_place(reply).error.unwrap();
    let said = format!("{}\n{}", error.message, error.hints.join("\n"));
    for want in [
        "1 unpushed commit",
        "2 uncommitted file",
        "exec_in_box",
        "git push",
        "tell your user",
    ] {
        assert!(said.contains(want), "missing {want:?} in {said}");
    }
    for cli in ["moor run", "moor cp", "--force", "moor close"] {
        assert!(!said.contains(cli), "{cli:?} in {said}");
    }
}

#[test]
fn a_start_from_a_place_carries_its_no_setup() {
    let w = world();
    let mut call = start(w.place, "o/n", &[]);
    if let BoxCall::Start { no_setup, .. } = &mut call.call {
        *no_setup = true;
    }
    let admitted = w.admit(&call).unwrap();
    assert!(matches!(admitted.plan, Plan::Start(ref r) if r.no_setup && r.no_agent));
    let plain = w.admit(&start(w.place, "o/n", &[])).unwrap();
    assert!(matches!(plain.plan, Plan::Start(ref r) if !r.no_setup));
}
