use super::*;

#[test]
fn errors_display() {
    assert_eq!(
        EngineError::Box(DriverError::NotFound).to_string(),
        "box: box not found"
    );
}

#[test]
fn launch_errors_display() {
    let failed = EngineError::Failed {
        step: "brief",
        message: "box file write failed".to_owned(),
    };
    assert_eq!(
        failed.to_string(),
        "launch failed at brief: box file write failed"
    );
    assert_eq!(EngineError::PaneGone.to_string(), "the agent pane is gone");
    assert_eq!(EngineError::NoSession.to_string(), "no agent session");
}

fn req() -> LaunchReq {
    LaunchReq {
        profile: None,
        mcp: std::collections::BTreeMap::new(),
        id: BoxId::new(7),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        workspace: WorkspaceRef::new("ws-1"),
        box_name: BoxName::parse_allow_bare("web/fix-bug").unwrap(),
        branch: String::new(),
        resume: None,
        issue: None,
        setup_exit: None,
        repo_notes: None,
        shape: None,
        model: None,
        nonce: "abc".to_owned(),
        proxy: None,
        strip_env: Vec::new(),
        prompt: LaunchPrompt::None,
    }
}

#[test]
fn dir_names_come_from_the_box_id() {
    let (cred, tmp) = req().dir_names();
    assert_eq!(
        (cred.as_str(), tmp.as_str()),
        (".moor-cred-7-abc", ".moor-tmp-7-abc")
    );
    assert!(!cred.contains('/') && !tmp.contains('/'));
}

fn proxied(box_port: u16, ca_pem: &str) -> LaunchReq {
    LaunchReq {
        profile: None,
        mcp: std::collections::BTreeMap::new(),
        proxy: Some(Arc::new(ProxyAddr {
            box_port,
            egress_port: box_port,
            ca_pem: ca_pem.to_owned(),
            box_env: Vec::new(),
            forge: false,
            hosts: Vec::new(),
        })),
        ..req()
    }
}

#[test]
fn launches_with_proxies_are_equal_exactly_when_the_box_ports_are() {
    assert_eq!(proxied(4000, "a"), proxied(4000, "b"));
    assert_ne!(proxied(4000, "a"), proxied(4001, "a"));
    assert_ne!(proxied(4000, "a"), req());
}
