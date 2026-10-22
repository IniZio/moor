//! Only provisioning clones the repo in a Box: an attaching start, a run and a close's teardown
//! run no clone exec.

use moor_core::{BoxTarget, CloseReq, RunReq};

use super::setup::{start_req, start_with};
use super::World;
use crate::error::CommandError;
use crate::ops::start::run;
use crate::testing::driver::{is_clone_exec, FakeDriver};

const YAML: &str = "teardown: [echo bye]\n";

/// An op under test, and whether it must reach the Box.
type Op<'a> = (&'a str, &'a dyn Fn() -> Result<(), CommandError>, bool);

/// Whether each exec the fake Driver received from `from` on is the clone, with its argv.
fn clones_since(w: &World, from: usize) -> Vec<(bool, Vec<String>)> {
    let execs = w.driver.execs();
    execs
        .get(from..)
        .unwrap_or_default()
        .iter()
        .map(|(_, spec, _)| (is_clone_exec(spec), spec.argv.clone()))
        .collect()
}

#[test]
fn only_provisioning_clones_the_repo_in_the_box() {
    let (w, _) = start_with(YAML, FakeDriver::new());
    let provisioned = clones_since(&w, 0);
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
        let from = w.driver.execs().len();
        let done = step();
        assert!(done.is_ok(), "{op}: {done:?}");
        let got = clones_since(&w, from);
        assert_eq!(!got.is_empty(), execs, "{op}: {got:?}");
        for (clone, argv) in &got {
            assert!(!clone, "{op} cloned for {:?}", argv.last());
        }
    }
}
