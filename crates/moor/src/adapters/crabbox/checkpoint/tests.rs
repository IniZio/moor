use std::sync::Arc;

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{Checkpoint, Driver, DriverError};
use moor_core::seam::proc::Tool;

use crate::adapters::crabbox::tests::req;
use crate::adapters::crabbox::Crabbox;
use crate::testing::proc::{Reply, ScriptedProc};

fn driver(verb: &str, reply: Reply) -> (Arc<ScriptedProc>, Crabbox) {
    let p = Arc::new(ScriptedProc::new());
    p.on(Tool::Crabbox, &["checkpoint", verb], reply);
    p.on(Tool::Crabbox, &["stop"], Reply::Ok(String::new()));
    let c = Crabbox::fake(p.clone());
    (p, c)
}

fn fail() -> Reply {
    Reply::Fail {
        code: 1,
        stdout: String::new(),
        stderr: "boom".to_owned(),
    }
}

#[test]
fn a_checkpoint_names_the_lease_and_layer_and_reads_the_id_and_size() {
    let (p, c) = driver(
        "create",
        Reply::Ok(r#"{"id":"cp-9","bytes":42}"#.to_owned()),
    );
    let handle = BoxHandle::new("local/moor-7-ab12cd");
    let got = c.checkpoint(&handle, "k1").unwrap();
    assert_eq!(
        got,
        Checkpoint {
            id: "cp-9".to_owned(),
            bytes: Some(42)
        }
    );
    let call = p.calls().pop().unwrap();
    assert_eq!(
        call.args,
        [
            "checkpoint",
            "create",
            "--provider",
            "local",
            "--id",
            "moor-7-ab12cd",
            "--name",
            "k1",
            "--mode",
            "native",
            "--json"
        ]
    );
    assert!(
        call.cwd.unwrap().ends_with("moor-7-ab12cd"),
        "run from the lease dir"
    );
}

#[test]
fn a_numeric_id_is_read_and_a_missing_one_is_unreadable() {
    let handle = BoxHandle::new("local/moor-7-ab12cd");
    let (_, c) = driver("create", Reply::Ok(r#"{"id":12}"#.to_owned()));
    assert_eq!(c.checkpoint(&handle, "k").unwrap().id, "12");
    let (_, c) = driver("create", Reply::Ok(r#"{"bytes":1}"#.to_owned()));
    assert!(matches!(
        c.checkpoint(&handle, "k"),
        Err(DriverError::Failed { .. })
    ));
}

#[test]
fn a_fork_starts_the_lease_moor_planned_from_its_own_dir() {
    let (p, c) = driver("fork", Reply::Ok(r#"{"slug":"moor-7-ab12cd"}"#.to_owned()));
    let r = req("local", None);
    let got = c.fork("cp-9", &r).unwrap();
    assert_eq!(got, c.planned_handle(&r));
    let call = p.calls().pop().unwrap();
    assert_eq!(
        call.args,
        [
            "checkpoint",
            "fork",
            "cp-9",
            "--slug",
            "moor-7-ab12cd",
            "--json"
        ]
    );
    assert!(call.cwd.unwrap().ends_with("moor-7-ab12cd"));
}

#[test]
fn a_fork_under_another_slug_or_a_failed_one_releases_what_it_started() {
    for reply in [Reply::Ok(r#"{"slug":"other"}"#.to_owned()), fail()] {
        let (p, c) = driver("fork", reply);
        assert!(c.fork("cp-9", &req("local", None)).is_err());
        let stops = p
            .calls()
            .iter()
            .filter(|c| c.args.first().is_some_and(|a| a == "stop"))
            .count();
        assert_eq!(stops, 1);
    }
}
