//! Whose setup checkpoints a Box may start from: only those its own user's Boxes saved.

use moor_core::boxscript::clone::AT_BASE;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::seam::forge::ForgeRepo;
use moor_core::user::Identity;
use moor_core::{BoxId, BoxName, BoxReq, StartReq};

use super::{exit, row, setup_execs, world, World, TWO_LINES};
use crate::ops::retry::run;
use crate::testing::cx::{put_secret, requester};
use crate::testing::driver::FakeDriver;
use crate::vault::Vault;

fn bob() -> Identity {
    Identity::local(2000)
}

/// Enrolls [`bob`] with the secrets a start needs.
fn enroll_bob(w: &World) {
    let store = w.cx.store().unwrap();
    let (user, _) = store.enroll(&bob(), None).unwrap();
    let vault = Vault::load_or_create(w.cx.vault_key().unwrap()).unwrap();
    for name in ["claude", "github"] {
        put_secret(store, &vault, user, name, &format!("{name}-of-2000"));
    }
}

/// Starts Box `name` of `o/n` with no agent, whether or not the start succeeds.
fn start(w: &World, name: &str) -> BoxId {
    let name = BoxName::parse_allow_bare(name).unwrap();
    let req = StartReq {
        name: name.clone(),
        prompt: None,
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: true,
        resume: false,
        repo: Some(ForgeRepo::new("o", "n")),
        parent: None,
        also: None,
        identity: None,
    };
    let _ = crate::ops::start::run(&w.cx, &req);
    w.cx.store()
        .unwrap()
        .open_box_by_name(&name)
        .unwrap()
        .unwrap()
        .id
}

/// Retries Box `name`, which then passes its setup.
fn retry(w: &World, name: &str) {
    let req = BoxReq {
        box_name: BoxName::parse_allow_bare(name).unwrap(),
    };
    let v = run(&w.cx, &req).unwrap();
    assert!(v.setup.ran && v.setup.ok);
}

/// Which setup lines ran on the machine Box `id` is on now.
fn lines_on_its_machine(w: &World, id: BoxId) -> Vec<(bool, bool)> {
    let handle = row(w, id).boxed.unwrap().handle.unwrap();
    setup_execs(w)
        .into_iter()
        .filter(|(h, _)| h == handle.as_str())
        .map(|(_, argv)| (argv.contains("prep-one"), argv.contains("run-two")))
        .collect()
}

#[test]
fn a_retry_never_forks_another_users_checkpoint_and_runs_those_lines_itself() {
    let driver = FakeDriver::new()
        .with_checkpoints()
        .exec_ends_once_when("run-two", exit(1));
    let mut w = world(TWO_LINES, driver);
    enroll_bob(&w);
    w.cx = w.cx.with_requester(bob());
    let bobs = start(&w, "bobs");
    assert_eq!(row(&w, bobs).lifecycle, BoxLifecycle::SetupFailed);
    w.cx = w.cx.with_requester(requester());
    let alices = start(&w, "alices");
    assert_eq!(row(&w, alices).lifecycle, BoxLifecycle::Ready);
    w.cx = w.cx.with_requester(bob());
    retry(&w, "bobs");
    assert_eq!(
        lines_on_its_machine(&w, bobs),
        [(true, false), (false, true)],
        "both lines ran again for Bob, none came from Alice's checkpoint"
    );
}

#[test]
fn a_retry_forks_a_checkpoint_its_own_users_box_saved() {
    let driver = FakeDriver::new()
        .with_checkpoints()
        .exec_ends_once_when("run-two", exit(1));
    let w = world(TWO_LINES, driver);
    let first = start(&w, "first");
    assert_eq!(row(&w, first).lifecycle, BoxLifecycle::SetupFailed);
    let second = start(&w, "second");
    assert_eq!(row(&w, second).lifecycle, BoxLifecycle::Ready);
    retry(&w, "first");
    assert!(w.driver.calls().iter().any(|c| c.starts_with("fork ")));
    assert_eq!(
        lines_on_its_machine(&w, first),
        Vec::<(bool, bool)>::new(),
        "the fork of the last layer leaves no line to run"
    );
}

/// Starts `demo` without checkpoints, loses its machine and starts it again on a provider that
/// keeps them, with the clone printing `clone_says`; returns the checkpoints saved.
fn relaunched(clone_says: &str) -> Vec<String> {
    let w = world(TWO_LINES, FakeDriver::new());
    let id = start(&w, "demo");
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::Ready);
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    w.driver.offer_checkpoints();
    w.driver.push_exec_stdout(clone_says);
    start(&w, "demo");
    assert_eq!(row(&w, id).lifecycle, BoxLifecycle::Ready);
    w.driver.checkpoint_names()
}

#[test]
fn a_relaunch_whose_clone_did_not_report_the_base_saves_no_checkpoint() {
    assert_eq!(relaunched("\n"), Vec::<String>::new());
}

#[test]
fn a_relaunch_whose_clone_started_from_the_base_saves_checkpoints() {
    assert!(!relaunched(&format!("{AT_BASE}\n")).is_empty());
}
