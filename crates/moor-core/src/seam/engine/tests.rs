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

#[test]
fn dir_names_come_from_the_box_id() {
    let req = LaunchReq {
        id: BoxId::new(7),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: PathBuf::from("/wt"),
        workspace: WorkspaceRef::new("ws-1"),
        task: BoxName::parse_allow_bare("web/fix-bug").unwrap(),
        branch: String::new(),
        goal: None,
        resume: None,
        issue: None,
        setup_exit: None,
        model: None,
        nonce: "abc".to_owned(),
        proxy: None,
        origin: None,
        strip_env: Vec::new(),
    };
    let (cred, tmp) = req.dir_names();
    assert_eq!(
        (cred.as_str(), tmp.as_str()),
        (".moor-cred-7-abc", ".moor-tmp-7-abc")
    );
    assert!(!cred.contains('/') && !tmp.contains('/'));
}
