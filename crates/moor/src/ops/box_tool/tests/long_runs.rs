//! Long commands in a work Box: a run, its answer, and who may wait on it.

use super::*;

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
    fn start(&mut self, place: BoxId, admitted: Admitted) -> BoxRunId {
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
            .start_box_run(place, req, || false)
    }

    /// The answer on `run` for `place` after up to `window`.
    fn answer(&self, place: BoxId, run: BoxRunId, window: Duration) -> Result<BoxRunView, String> {
        runs::answer(&self.f.cx, place, run, window, &|| false).map_err(|e| {
            assert_eq!(e.kind, ErrorKind::Refused, "{}", e.error);
            e.error.to_string()
        })
    }
}

#[test]
fn a_short_command_answers_with_its_exit_and_output() {
    let mut w = world();
    let admitted = w.admit(&exec(w.place, "t")).unwrap();
    let run = w.start(w.place, admitted);
    let view = w.answer(w.place, run, runs::WINDOW).unwrap();
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
    let run = w.start(w.place, admitted);
    let clock = w.f.cx.clock_arc();
    let first = std::thread::scope(|s| {
        s.spawn(|| {
            for _ in 0..200 {
                std::thread::sleep(Duration::from_millis(20));
                clock.sleep(Duration::from_secs(1));
            }
        });
        w.answer(w.place, run, runs::WINDOW).unwrap()
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
    let end = w.answer(w.place, run, runs::WINDOW).unwrap();
    assert_eq!(end.state, BoxRunState::Exited { exit: 0 }, "{end:?}");
    assert_eq!(
        end.stdout, "done\n",
        "only what came after the first answer"
    );
    let again = w.answer(w.place, run, runs::WINDOW).unwrap();
    assert_eq!(again.state, BoxRunState::Exited { exit: 0 });
    assert_eq!(again.stdout, "");
}

#[test]
fn a_wait_on_another_user_s_run_is_refused() {
    let mut w = world();
    let admitted = w.admit(&exec(w.place, "t")).unwrap();
    let run = w.start(w.place, admitted);
    let theirs = w.admit(&wait(w.their_place, run)).unwrap();
    assert_ne!(theirs.user, REQUESTER_USER);
    let why = w.answer(w.their_place, run, Duration::ZERO).unwrap_err();
    assert!(why.contains("no such run"), "{why}");
    let none = w
        .answer(w.place, BoxRunId(run.0 ^ 1), Duration::ZERO)
        .unwrap_err();
    assert_eq!(none, why, "another user's run reads as no run at all");
    assert!(w.answer(w.place, run, runs::WINDOW).is_ok());
}

#[test]
fn a_wait_from_another_task_s_place_of_the_same_user_is_refused() {
    let mut w = world();
    let admitted = w.admit(&exec(w.place, "t")).unwrap();
    let run = w.start(w.place, admitted);
    let sibling = w.admit(&wait(w.other_place, run)).unwrap();
    assert_eq!(sibling.user, REQUESTER_USER);
    let why = w.answer(w.other_place, run, Duration::ZERO).unwrap_err();
    assert!(why.contains("no such run"), "{why}");
    assert!(w.answer(w.place, run, runs::WINDOW).is_ok());
}

#[test]
fn a_wait_from_a_work_box_or_an_idle_place_is_refused() {
    let w = world();
    refused_with(w.admit(&wait(w.f.id, BoxRunId(1))), "this is a work Box");
    refused_with(w.admit(&wait(w.idle, BoxRunId(1))), "once a task claims it");
}
