//! Only provisioning clones the repo in a Box: an attaching start, a run and a close's teardown
//! run no clone exec.

use std::path::Path;

use moor_core::seam::proc::Tool;
use moor_core::{BoxId, BoxTarget, CloseReq, RunReq};

use super::moor_yaml::script_base_yaml;
use super::setup::{start_req, start_with};
use super::World;
use crate::error::CommandError;
use crate::ops::start::run;
use crate::testing::driver::{is_clone_exec, FakeDriver};
use crate::testing::proc::Reply;

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

/// Leaves the teardown config in the Box's host repo and on its base commit.
fn keep_teardown(w: &World, id: BoxId) {
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    let wt = row.repo;
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(Path::new(&wt).join(".moor.yaml"), YAML).unwrap();
    script_base_yaml(w, Some(YAML));
}

#[test]
fn only_provisioning_clones_the_repo_in_the_box() {
    let (w, v) = start_with(YAML, FakeDriver::new());
    let id = BoxId::new(v.boxed.id);
    let provisioned = clones_since(&w, 0);
    assert!(
        provisioned.iter().any(|(clone, _)| *clone),
        "{provisioned:?}"
    );
    let name = start_req().name;
    let attach = || {
        keep_teardown(&w, id);
        run(&w.cx, &start_req()).map(drop)
    };
    let run_once = || {
        keep_teardown(&w, id);
        let req = RunReq {
            box_name: name.clone(),
            record: None,
            argv: vec!["true".to_owned()],
            trust_moor_yaml: false,
            inherit_output: false,
        };
        crate::ops::run::run(&w.cx, &req).map(drop)
    };
    let close = || {
        keep_teardown(&w, id);
        w.proc.on(Tool::Git, &["rev-list"], Reply::Ok("0\n".into()));
        w.proc.on(Tool::Git, &["status"], Reply::Ok(String::new()));
        let req = CloseReq {
            target: BoxTarget::Name(name.clone()),
            force: true,
            trust_moor_yaml: false,
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
