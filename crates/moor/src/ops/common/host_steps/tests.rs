//! Tests for the host steps.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;

use moor_core::config::{Flags, TaskSettings, UserConfig};
use moor_core::domain::decision::{Decision, NewTask};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::task_key::TaskKey;
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};

use super::parse::{classify, holder, Clash};
use super::{backend_up, HostSite};
use crate::cx::Cx;
use crate::store::lease::{LeaseKind, LeaseState};
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const PORT_ERR: &str = "Error response from daemon: driver failed programming external \
connectivity on endpoint web-1: Bind for 0.0.0.0:8080 failed: port is already allocated\n";
const NAME_ERR: &str = "Error response from daemon: Conflict. The container name \"/web\" is \
already in use by container \"abc123def\". You have to remove (or rename) that container.\n";

struct Rig {
    dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: TaskId,
    handle: BoxHandle,
    workdir: BoxPath,
    key: TaskKey,
}

fn rig(driver: FakeDriver) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        Arc::clone(&driver) as Arc<_>,
        Arc::new(FakeSurface::new(dir.path())),
        Arc::new(ScriptedProc::new()),
    );
    let name = TaskName::parse("demo").unwrap();
    let id = cx
        .store()
        .unwrap()
        .apply(Decision::insert(NewTask {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: id,
            name: name.clone(),
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    Rig {
        dir,
        cx,
        driver,
        id,
        handle: got.handle,
        workdir: got.workdir,
        key: TaskKey::new(&name),
    }
}

fn up(r: &Rig) -> Result<(), String> {
    let cfg = TaskSettings::resolve(&UserConfig::default(), None, &Flags::default());
    let site = HostSite {
        handle: &r.handle,
        workdir: &r.workdir,
        key: &r.key,
        worktree: r.dir.path(),
    };
    backend_up(&r.cx, r.id, &site, &cfg).map_err(|e| e.error.to_string())
}

fn exit(code: i32) -> ExecOutcome {
    ExecOutcome::Exited { code }
}

#[test]
fn up_runs_plain_compose_argv_in_the_workdir_without_the_sandbox() {
    let r = rig(FakeDriver::new());
    up(&r).unwrap();
    let execs = r.driver.execs();
    assert_eq!(execs.len(), 1);
    let (handle, spec, _) = &execs[0];
    assert_eq!(handle, &r.handle);
    assert_eq!(
        spec.argv,
        ["docker", "compose", "-p", "moor-demo", "up", "-d"]
    );
    assert_eq!(spec.cwd.as_ref(), Some(&r.workdir));
    assert!(spec.env.is_empty());
    assert!(spec.argv.iter().all(|a| !a.contains("nono")));
}

#[test]
fn up_holds_a_compose_lease_named_for_the_project() {
    let r = rig(FakeDriver::new());
    up(&r).unwrap();
    let rows = r.cx.store().unwrap().leases(r.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, LeaseKind::Compose);
    assert_eq!(rows[0].key, "moor-demo");
    assert_eq!(rows[0].state, LeaseState::Held);
}

#[test]
fn lease_is_planned_before_the_exec_runs() {
    let r = rig(FakeDriver::new()
        .exec_results([Err(DriverError::Timeout(std::time::Duration::from_secs(1)))]));
    assert!(up(&r).is_err());
    let rows = r.cx.store().unwrap().leases(r.id).unwrap();
    assert_eq!(rows[0].key, "moor-demo");
    assert_eq!(rows[0].state, LeaseState::Planned);
}

#[test]
fn port_clash_names_the_port_and_the_holder() {
    let ps = "other-web-1\tother\n";
    let r = rig(FakeDriver::new()
        .exec_results([Ok(exit(1)), Ok(exit(0))])
        .exec_stdout([PORT_ERR, ps]));
    assert_eq!(
        up(&r).unwrap_err(),
        "host step backend: port 8080 is held by container \"other-web-1\" \
         (compose project \"other\"); stop it or change the port"
    );
    let execs = r.driver.execs();
    assert_eq!(
        execs[1].1.argv[..4],
        ["docker", "ps", "--filter", "publish=8080"]
    );
    let rows = r.cx.store().unwrap().leases(r.id).unwrap();
    assert_eq!(rows[0].state, LeaseState::Planned);
}

#[test]
fn port_clash_without_a_holder_still_names_the_port() {
    let r = rig(FakeDriver::new()
        .exec_results([Ok(exit(1)), Ok(exit(0))])
        .exec_stdout([PORT_ERR, ""]));
    assert_eq!(
        up(&r).unwrap_err(),
        "host step backend: port 8080 is held by another process or container; \
         stop it or change the port"
    );
}

#[test]
fn name_clash_names_the_container_and_its_holder() {
    let r = rig(FakeDriver::new()
        .exec_results([Ok(exit(1))])
        .exec_stdout([NAME_ERR]));
    assert_eq!(
        up(&r).unwrap_err(),
        "host step backend: container name \"web\" is already in use by container \
         \"abc123def\"; remove that container or rename the service"
    );
}

#[test]
fn other_failure_shows_the_last_stderr_line() {
    let r = rig(FakeDriver::new()
        .exec_results([Ok(exit(1))])
        .exec_stdout(["pulling\nno configuration file provided: not found\n"]));
    assert_eq!(
        up(&r).unwrap_err(),
        "host step backend: docker compose -p moor-demo up -d failed: \
         no configuration file provided: not found"
    );
}

#[test]
fn driver_error_is_reported_as_the_failure() {
    let r = rig(FakeDriver::new().exec_results([Err(DriverError::Unavailable("gone".into()))]));
    let msg = up(&r).unwrap_err();
    assert!(msg.starts_with("host step backend: docker compose -p moor-demo up -d failed: "));
}

#[test]
fn classify_reads_docker_texts() {
    let cases = [
        ("(bind) Bind for 0.0.0.0:8080 failed: port is already allocated", Clash::Port("8080".into())),
        ("(v6) Bind for [::]:5432 failed: port is already allocated", Clash::Port("5432".into())),
        (
            "(in use) listen tcp4 0.0.0.0:3000: bind: address already in use",
            Clash::Port("3000".into()),
        ),
        (
            "(name) Conflict. The container name \"/web\" is already in use by container \"abc\". x",
            Clash::Name { name: "web".into(), holder: Some("abc".into()) },
        ),
        (
            "(no holder) Conflict. The container name \"/db\" is already in use",
            Clash::Name { name: "db".into(), holder: None },
        ),
        ("(other) manifest unknown", Clash::Other),
    ];
    for (input, want) in cases {
        assert_eq!(classify(input), want, "{input}");
    }
}

#[test]
fn holder_reads_the_first_ps_line() {
    assert_eq!(holder(""), None);
    assert_eq!(holder("a\tp\nb\tq\n"), Some(("a".into(), Some("p".into()))));
    assert_eq!(holder("solo\t\n"), Some(("solo".into(), None)));
    assert_eq!(holder("solo\n"), Some(("solo".into(), None)));
}
