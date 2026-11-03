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

/// What crabbox prints for a checkpoint it made.
const CREATED: &str = r#"{"captureEstablished":true,"id":"chk_61359ad7f939edd9","name":"after-clone","kind":"external-snapshot","createdAt":"2026-11-03T04:43:14Z","lastUsedAt":"2026-11-03T04:43:14Z","provider":"external","leaseId":"cbx_c1cc0166642e","slug":"base","workdir":"/w/my-app","native":{"provider":"external","imageId":"chk_1","kind":"external-snapshot","name":"after-clone","state":"available","bytes":4096,"direct":true}}"#;

/// What crabbox prints for the lease it forked.
const FORKED: &str = r#"{"checkpointId":"chk_1","leaseId":"cbx_1b06599bda9e","slug":"moor-7-ab12cd","provider":"external","workdir":"/w/my-app"}"#;

#[test]
fn a_checkpoint_names_the_lease_and_layer_and_reads_the_id_and_size() {
    let (p, c) = driver("create", Reply::Ok(CREATED.to_owned()));
    let handle = BoxHandle::new("local/moor-7-ab12cd");
    let got = c.checkpoint(&handle, "k1").unwrap();
    assert_eq!(
        got,
        Checkpoint {
            id: "chk_61359ad7f939edd9".to_owned(),
            bytes: Some(4096)
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
    let (_, c) = driver("create", Reply::Ok(r#"{"native":{"bytes":1}}"#.to_owned()));
    assert!(matches!(
        c.checkpoint(&handle, "k"),
        Err(DriverError::Failed { .. })
    ));
}

#[test]
fn a_fork_starts_the_lease_moor_planned_from_its_own_dir() {
    let (p, c) = driver("fork", Reply::Ok(FORKED.to_owned()));
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
    let not_found = Reply::Fail {
        code: 1,
        stdout: String::new(),
        stderr: "checkpoint not found".to_owned(),
    };
    for reply in [
        Reply::Ok(r#"{"slug":"other"}"#.to_owned()),
        fail(),
        not_found,
    ] {
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
