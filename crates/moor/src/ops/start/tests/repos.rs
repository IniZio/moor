//! Provisioning a Box that lists repos after its primary one.

use moor_core::domain::decision::ListedRepo;
use moor_core::name::ForgeRepo;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::BoxId;

use super::{repo_req, script_base_yaml, world, World};
use crate::ops::common::provision::{reprovision, Provisioned};
use crate::ops::common::setup::SetupRun;
use crate::ops::start::run;
use crate::testing::driver::{is_clone_exec, FakeDriver};

fn listed(owner: &str, name: &str, setup: Option<&str>) -> ListedRepo {
    ListedRepo {
        repo: ForgeRepo::new(owner, name),
        moor_yaml: setup.map(|line| format!("setup: [{line}]\n")),
    }
}

fn lib_and_deploy() -> Vec<ListedRepo> {
    vec![
        listed("o", "lib", Some("lib-setup")),
        listed("o", "deploy", Some("deploy-setup")),
    ]
}

/// Starts Box 1 of `o/n` with `setup: [n-setup]`, its first acquire failing, lists `others`
/// after it and provisions it again.
fn provisioned(driver: FakeDriver, others: Vec<ListedRepo>) -> (World, Provisioned) {
    let (w, done) = reprovisioned(driver, others);
    (w, done.unwrap())
}

fn reprovisioned(
    driver: FakeDriver,
    others: Vec<ListedRepo>,
) -> (World, Result<Provisioned, String>) {
    let failing = driver.fail_acquire(DriverError::Failed {
        summary: "no machine".into(),
    });
    let w = world(failing);
    script_base_yaml(&w, Some("setup: [n-setup]\n"));
    run(&w.cx, &repo_req("demo")).unwrap_err();
    let store = w.cx.store().unwrap();
    store.list_others(BoxId::new(1), others).unwrap();
    let row = store.box_row(BoxId::new(1)).unwrap().unwrap();
    let done = reprovision(&w.cx, store, &row, "test").map(Option::unwrap);
    (w, done.map_err(|e| format!("{e:?}")))
}

fn script(spec: &ExecSpec) -> String {
    spec.argv.last().cloned().unwrap_or_default()
}

/// Each exec's kind and the work tree or repo it names, in run order.
fn steps(w: &World) -> Vec<String> {
    let workdir =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(1))
            .unwrap()
            .unwrap()
            .boxed
            .unwrap()
            .workdir
            .unwrap();
    let parent = workdir.rsplit_once('/').unwrap().0;
    let trees = [
        (format!("{parent}/lib"), "lib"),
        (format!("{parent}/deploy"), "deploy"),
        (workdir.clone(), "n"),
    ];
    w.driver
        .execs()
        .iter()
        .filter_map(|(_, spec, _)| {
            let text = format!("{spec:?}");
            let tree = trees
                .iter()
                .find(|(dir, _)| text.contains(dir.as_str()))
                .map_or("?", |(_, name)| name);
            if is_clone_exec(spec) {
                let url = ["o/lib", "o/deploy", "o/n"]
                    .into_iter()
                    .find(|u| text.contains(&format!("github.com/{u}.git")))?;
                Some(format!("clone {url} in {tree}"))
            } else {
                let line = ["n-setup", "lib-setup", "deploy-setup"]
                    .into_iter()
                    .find(|l| text.contains(l))?;
                Some(format!("setup {line} in {tree}"))
            }
        })
        .collect()
}

#[test]
fn every_listed_repo_is_cloned_on_the_box_branch_and_set_up_in_its_own_tree() {
    let (w, done) = provisioned(FakeDriver::new(), lib_and_deploy());
    let mut ran = steps(&w);
    // The other repos clone at the same time, so their relative order is not fixed.
    if let Some(others) = ran.get_mut(1..3) {
        others.sort();
    }
    assert_eq!(
        ran,
        [
            "clone o/n in n",
            "clone o/deploy in deploy",
            "clone o/lib in lib",
            "setup n-setup in n",
            "setup lib-setup in lib",
            "setup deploy-setup in deploy",
        ]
    );
    let lib = w
        .driver
        .execs()
        .into_iter()
        .map(|e| script(&e.1))
        .find(|s| s.contains("github.com/o/lib.git"))
        .unwrap();
    assert!(lib.contains("'moor/demo'"), "{lib}");
    assert!(!done.setup.failed(), "{:?}", done.setup);
}

#[test]
fn a_failing_setup_in_a_listed_repo_fails_the_box_setup_naming_that_repo() {
    let driver = FakeDriver::new().exec_ends_when("lib-setup", ExecOutcome::Exited { code: 1 });
    let (w, done) = provisioned(driver, lib_and_deploy());
    let SetupRun::Failed { line, .. } = &done.setup else {
        unreachable!("{:?}", done.setup)
    };
    assert_eq!(line, "o/lib: lib-setup");
    let ran = steps(&w);
    assert!(!ran.iter().any(|s| s.contains("deploy-setup")), "{ran:?}");
    let row =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(1))
            .unwrap()
            .unwrap();
    assert!(row.setup_failed_at.is_some(), "{row:?}");
}

#[test]
fn a_box_of_one_repo_clones_and_sets_up_only_its_workdir() {
    let (w, done) = provisioned(FakeDriver::new(), Vec::new());
    assert_eq!(steps(&w), ["clone o/n in n", "setup n-setup in n"]);
    assert!(!done.setup.failed(), "{:?}", done.setup);
}

#[test]
fn a_listed_repo_whose_tree_is_taken_fails_the_start_naming_both_repos() {
    let others = vec![listed("o", "lib", None), listed("p", "lib", None)];
    let (w, done) = reprovisioned(FakeDriver::new(), others);
    let shown = done.err().unwrap_or_default();
    assert!(
        shown.contains("o/lib and p/lib would both be cloned into"),
        "{shown}"
    );
    assert!(
        !steps(&w).iter().any(|s| s.contains("setup")),
        "{:?}",
        steps(&w)
    );
}

#[test]
fn the_other_repos_are_cloned_at_the_same_time() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    let driver = FakeDriver::new();
    let started = Arc::new(AtomicUsize::new(0));
    let waited_in_vain = Arc::new(AtomicBool::new(false));
    let (seen, lost) = (started, waited_in_vain.clone());
    // The primary's clone is the first exec; the first other clone holds until the second begins.
    driver.on_exec(move || {
        if seen.fetch_add(1, Ordering::SeqCst) + 1 != 2 {
            return;
        }
        for _ in 0..600 {
            if seen.load(Ordering::SeqCst) >= 3 {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        lost.store(true, Ordering::SeqCst);
    });
    let (_w, done) = provisioned(driver, lib_and_deploy());
    assert!(
        !waited_in_vain.load(Ordering::SeqCst),
        "the second clone did not start while the first was running"
    );
    assert!(!done.setup.failed(), "{:?}", done.setup);
}

#[test]
fn every_failed_clone_is_named_with_its_reason_in_listed_order() {
    let driver = FakeDriver::new()
        .exec_ends_when("o/lib.git", ExecOutcome::Exited { code: 1 })
        .exec_ends_when("o/deploy.git", ExecOutcome::Exited { code: 7 });
    let (_w, done) = reprovisioned(driver, lib_and_deploy());
    let shown = done.err().unwrap_or_default();
    let lib = shown.find("o/lib: ");
    let deploy = shown.find("o/deploy: ");
    assert!(lib.is_some() && deploy.is_some(), "{shown}");
    assert!(lib < deploy, "{shown}");
    assert!(shown.contains("; "), "{shown}");
}
