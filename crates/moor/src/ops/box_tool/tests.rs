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
use moor_core::user::{Caller, Identity, UserId, Viewer};
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
    }
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

/// `command` in work Box `t`, from Box `from`.
fn exec_of(from: BoxId, command: &str) -> BoxToolReq {
    BoxToolReq {
        from,
        call: BoxCall::Exec {
            target: name("t"),
            command: command.to_owned(),
            timeout: BoxExecSecs::new(Some(1800)).unwrap(),
        },
    }
}

/// A wait on `run`, from Box `from`.
fn wait(from: BoxId, run: BoxRunId) -> BoxToolReq {
    BoxToolReq {
        from,
        call: BoxCall::Wait { run },
    }
}

impl World {
    /// Starts the exec `admitted` plans as the controller does, on the fixture's context; a
    /// fresh context on the same state takes its place for the calls after.
    fn start(&mut self, admitted: Admitted) -> BoxRunId {
        let req = match admitted.plan {
            Plan::Exec(req) => Some(req),
            Plan::Start(_) | Plan::Close(_) | Plan::Wait(_) => None,
        }
        .unwrap();
        let answers = Cx::fake(
            self.f.dir.path(),
            self.f.driver.clone(),
            Arc::new(ScriptedProc::new()),
        )
        .with_env(|e| e.with_front_box_tools(BTreeSet::from(["gate".to_owned()])));
        let cx = std::mem::replace(&mut self.f.cx, answers);
        Moor::from_cx(cx)
            .granted(Grant {
                viewer: Viewer::User(admitted.user),
                pins: Pins::new(),
            })
            .start_box_run(admitted.user, req, || false)
    }

    /// The answer on `run` for `user` after up to `window`.
    fn answer(&self, user: UserId, run: BoxRunId, window: Duration) -> Result<BoxRunView, String> {
        runs::answer(&self.f.cx, user, run, window, &|| false).map_err(|e| {
            assert_eq!(e.kind, ErrorKind::Refused, "{}", e.error);
            e.error.to_string()
        })
    }
}

#[test]
fn a_short_command_answers_with_its_exit_and_output() {
    let mut w = world();
    let admitted = w.admit(&exec(w.place, "t")).unwrap();
    let run = w.start(admitted);
    let view = w.answer(REQUESTER_USER, run, runs::WINDOW).unwrap();
    assert_eq!(view.state, BoxRunState::Exited { exit: 0 }, "{view:?}");
    assert!(view.stdout.starts_with("hi from "), "{view:?}");
}

#[test]
fn a_long_command_answers_with_its_run_then_its_exit_on_wait() {
    let mut w = world();
    let go = w.f.dir.path().join("go");
    let command = format!(
        "echo started; while [ ! -e '{}' ]; do sleep 0.05; done; echo done",
        go.display()
    );
    let admitted = w.admit(&exec_of(w.place, &command)).unwrap();
    let run = w.start(admitted);
    let clock = w.f.cx.clock_arc();
    let first = std::thread::scope(|s| {
        s.spawn(|| {
            for _ in 0..200 {
                std::thread::sleep(Duration::from_millis(20));
                clock.sleep(Duration::from_secs(1));
            }
        });
        w.answer(REQUESTER_USER, run, runs::WINDOW).unwrap()
    });
    assert_eq!(
        (first.run, first.state),
        (run, BoxRunState::Running),
        "{first:?}"
    );
    assert_eq!(first.stdout, "started\n");
    let admitted = w.admit(&wait(w.place, run)).unwrap();
    assert_eq!(admitted.plan, Plan::Wait(run));
    std::fs::write(&go, "").unwrap();
    let end = w.answer(admitted.user, run, runs::WINDOW).unwrap();
    assert_eq!(end.state, BoxRunState::Exited { exit: 0 }, "{end:?}");
    assert_eq!(
        end.stdout, "done\n",
        "only what came after the first answer"
    );
    let again = w.answer(admitted.user, run, runs::WINDOW).unwrap();
    assert_eq!(again.state, BoxRunState::Exited { exit: 0 });
    assert_eq!(again.stdout, "");
}

#[test]
fn a_wait_on_another_user_s_run_is_refused() {
    let mut w = world();
    let admitted = w.admit(&exec(w.place, "t")).unwrap();
    let run = w.start(admitted);
    let theirs = w.admit(&wait(w.their_place, run)).unwrap();
    assert_ne!(theirs.user, REQUESTER_USER);
    let why = w.answer(theirs.user, run, Duration::ZERO).unwrap_err();
    assert!(why.contains("no such run"), "{why}");
    let none = w
        .answer(REQUESTER_USER, BoxRunId(run.0 ^ 1), Duration::ZERO)
        .unwrap_err();
    assert_eq!(none, why, "another user's run reads as no run at all");
    assert!(w.answer(REQUESTER_USER, run, runs::WINDOW).is_ok());
}

#[test]
fn a_wait_from_a_work_box_or_an_idle_place_is_refused() {
    let w = world();
    refused_with(w.admit(&wait(w.f.id, BoxRunId(1))), "this is a work Box");
    refused_with(w.admit(&wait(w.idle, BoxRunId(1))), "once a task claims it");
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
