use std::sync::Arc;

use moor_core::name::{BoxHandle, ProviderId};
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
            bytes: Some(4096),
            parent: None,
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
fn a_delta_checkpoint_reads_its_parent_and_counts_only_its_own_bytes() {
    let handle = BoxHandle::new("local/moor-7-ab12cd");
    let delta = r#"{"id":"chk_2","native":{"parentCheckpointId":"chk_1","uniqueBytes":100,"apparentBytes":5000,"bytes":4096}}"#;
    let (_, c) = driver("create", Reply::Ok(delta.to_owned()));
    assert_eq!(
        c.checkpoint(&handle, "k").unwrap(),
        Checkpoint {
            id: "chk_2".to_owned(),
            bytes: Some(100),
            parent: Some("chk_1".to_owned()),
        }
    );
}

#[test]
fn a_fork_starts_the_lease_moor_planned_from_its_own_dir() {
    let (p, c) = driver("fork", Reply::Ok(FORKED.to_owned()));
    let r = req("local", Some("small"));
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
            "--class",
            "standard",
            "--json"
        ]
    );
    assert!(call.cwd.unwrap().ends_with("moor-7-ab12cd"));
}

#[test]
fn a_fork_of_a_box_with_no_size_passes_no_class() {
    let (p, c) = driver("fork", Reply::Ok(FORKED.to_owned()));
    c.fork("cp-9", &req("local", None)).unwrap();
    let args = p.calls().pop().unwrap().args;
    assert!(!args.iter().any(|a| a == "--class"), "{args:?}");
}

#[test]
fn a_fork_takes_the_class_of_the_box_size() {
    for (size, class) in [("medium", "fast"), ("large", "large"), ("xlarge", "beast")] {
        let (p, c) = driver("fork", Reply::Ok(FORKED.to_owned()));
        c.fork("cp-9", &req("local", Some(size))).unwrap();
        let args = p.calls().pop().unwrap().args;
        let at = args.iter().position(|a| a == "--class").unwrap();
        assert_eq!(args.get(at + 1).map(String::as_str), Some(class));
    }
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

#[test]
fn a_delete_names_the_checkpoint() {
    let (p, c) = driver(
        "delete",
        Reply::Ok("checkpoint deleted id=chk_1\n".to_owned()),
    );
    c.delete_checkpoint(&ProviderId::new("x"), "chk_1").unwrap();
    assert_eq!(
        p.calls().pop().unwrap().args,
        ["checkpoint", "delete", "chk_1"]
    );
}

#[test]
fn the_disk_is_measured_for_a_provider_on_this_host_only() {
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(ScriptedProc::new());
    let providers = r#"[{"provider":"external","category":"external-provider"},
        {"provider":"hetzner","category":"brokerable-cloud"}]"#;
    p.on(
        Tool::Crabbox,
        &["providers"],
        Reply::Ok(providers.to_owned()),
    );
    let c = Crabbox::fake(p).with_checkpoint_dir(Some(dir.path().join("not-made-yet")));
    let disk = c.checkpoint_disk(&ProviderId::new("external")).unwrap();
    assert!(disk.is_some_and(|d| d.total_mib > 0 && d.free_mib <= d.total_mib));
    let remote = c.checkpoint_disk(&ProviderId::new("hetzner")).unwrap();
    assert_eq!(remote, None, "a cloud provider's disk is not this host's");
}
