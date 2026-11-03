//! The config, controller and gate checks.

use super::*;

#[test]
fn config_warns_when_no_controller_is_set() {
    let c = Rig::new().with(setup::config);
    assert_eq!(c.status, Status::Warn);
    assert!(c.detail.contains("/nonexistent/moor.yaml"), "{c:?}");
    assert_eq!(c.fix, "set controller.socket to use a running controller");
}

#[test]
fn config_names_the_file_it_read_and_the_controller() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("config.yaml");
    std::fs::write(&file, "nodes: {}\n").unwrap();
    let mut rig = Rig::new().with_controller(Box::new(|r| answered(r.cmd(), json!({}))));
    rig.env = Env::from_pairs([("MOOR_CONFIG", file.display().to_string())]);
    let c = rig.with(setup::config);
    assert_eq!(c.status, Status::Ok);
    assert_eq!(
        c.detail,
        format!("read {}; controller /run/moor.sock", file.display())
    );
}

#[test]
fn config_fails_with_the_parse_error() {
    let mut rig = Rig::new();
    rig.config_error = Some("nodes.x.runtime: empty".to_owned());
    let c = rig.with(setup::config);
    assert_eq!(c.status, Status::Fail);
    assert!(c.detail.contains("nodes.x.runtime"));
}

#[test]
fn controller_reports_a_matching_version() {
    let rig = Rig::new().with_controller(Box::new(|r| {
        answered(r.cmd(), json!({"version": env!("CARGO_PKG_VERSION")}))
    }));
    let c = rig.with(setup::controller);
    assert_eq!(c.status, Status::Ok);
    assert_eq!(
        c.detail,
        format!("moor {} at /run/moor.sock", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn controller_warns_on_another_version() {
    let rig =
        Rig::new().with_controller(Box::new(|r| answered(r.cmd(), json!({"version": "0.0.1"}))));
    let c = rig.with(setup::controller);
    assert_eq!(c.status, Status::Warn);
    assert!(c.detail.contains("0.0.1") && c.detail.contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn unreachable_controller_fails_with_the_clients_hint() {
    let rig = Rig::new().with_controller(Box::new(|_| {
        Err(ClientError::Unreachable {
            target: Target::Socket("/run/moor.sock".into()),
            detail: "no such file".to_owned(),
        })
    }));
    let c = rig.with(setup::controller);
    assert_eq!(c.status, Status::Fail);
    assert!(c.detail.contains("unreachable"), "{c:?}");
    assert!(c.fix.contains("moor controller serve"), "{c:?}");
}

#[test]
fn controller_check_warns_without_a_target() {
    assert_eq!(Rig::new().with(setup::controller).status, Status::Warn);
}

#[test]
fn gate_answering_200_is_ok() {
    let mut rig = Rig::new();
    rig.public_url = Some("https://moor.example/".to_owned());
    rig.get = Box::new(|url| {
        assert_eq!(
            url,
            "https://moor.example/.well-known/oauth-protected-resource"
        );
        Ok(200)
    });
    assert_eq!(rig.with(gate::check).map(|c| c.status), Some(Status::Ok));
}

#[test]
fn gate_not_answering_warns_to_run_it() {
    let mut rig = Rig::new();
    rig.public_url = Some("https://moor.example".to_owned());
    rig.get = Box::new(|_| Err("connection refused".to_owned()));
    let c = rig.with(gate::check).unwrap();
    assert_eq!(c.status, Status::Warn);
    assert_eq!(c.fix, "run moor-gate to let teammates connect");
    rig.get = Box::new(|_| Ok(404));
    assert_eq!(rig.with(gate::check).unwrap().status, Status::Warn);
}
