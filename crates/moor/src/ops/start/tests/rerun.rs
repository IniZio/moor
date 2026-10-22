//! Only provisioning clones the repo in a Box: an attaching start, a run and a close's teardown
//! send the Box no clone.

use moor_core::{BoxTarget, CloseReq, RunReq};

use super::setup::{start_req, start_with};
use super::World;
use crate::error::CommandError;
use crate::ops::start::run;
use crate::testing::driver::{is_clone_exec, FakeDriver};

const YAML: &str = "teardown: [echo bye]\n";

/// An op under test, and whether it must reach the Box.
type Op<'a> = (&'a str, &'a dyn Fn() -> Result<(), CommandError>, bool);

/// How many execs and run launches the fake Driver has received.
fn sent(w: &World) -> (usize, usize) {
    (w.driver.execs().len(), w.driver.launches().len())
}

/// Whether each exec and run launch the fake Driver received since `from` is the clone, with
/// its argv.
fn clones_since(w: &World, (execs, launches): (usize, usize)) -> Vec<(bool, Vec<String>)> {
    let e = w.driver.execs();
    let l = w.driver.launches();
    let from_execs = e.get(execs..).unwrap_or_default().iter().map(|(_, s, _)| s);
    let from_launches = l.get(launches..).unwrap_or_default().iter().map(|(_, s)| s);
    from_execs
        .chain(from_launches)
        .map(|spec| (is_clone_exec(spec), spec.argv.clone()))
        .collect()
}

#[test]
fn only_provisioning_clones_the_repo_in_the_box() {
    let box_dir = tempfile::tempdir().unwrap();
    let (w, _) = start_with(YAML, FakeDriver::new().local_box(box_dir.path()));
    let provisioned = clones_since(&w, (0, 0));
    assert!(
        provisioned.iter().any(|(clone, _)| *clone),
        "{provisioned:?}"
    );
    let name = start_req().name;
    let attach = || run(&w.cx, &start_req()).map(drop);
    let run_once = || {
        let req = RunReq {
            box_name: name.clone(),
            record: None,
            argv: vec!["true".to_owned()],
            inherit_output: false,
        };
        crate::ops::run::run(&w.cx, &req).map(drop)
    };
    let close = || {
        let req = CloseReq {
            target: BoxTarget::Name(name.clone()),
            force: true,
        };
        crate::ops::close::run(&w.cx, &req).map(drop)
    };
    let ops: [Op<'_>; 3] = [
        ("attaching start", &attach, false),
        ("run", &run_once, true),
        ("close", &close, true),
    ];
    for (op, step, execs) in ops {
        let from = sent(&w);
        let done = step();
        assert!(done.is_ok(), "{op}: {done:?}");
        let got = clones_since(&w, from);
        assert_eq!(!got.is_empty(), execs, "{op}: {got:?}");
        for (clone, argv) in &got {
            assert!(!clone, "{op} cloned for {:?}", argv.last());
        }
    }
}
