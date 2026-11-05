use super::Moor;
use crate::env::Env;
use moor_core::{CloseReq, ExitClass, Request};

/// An engine on a fresh state dir, which lives as long as the returned guard.
fn moor() -> (tempfile::TempDir, Moor) {
    let dir = tempfile::tempdir().expect("tmp");
    let env = Env::from_pairs([
        ("MOOR_STATE_DIR", dir.path().to_str().expect("utf8")),
        ("MOOR_CONFIG", "/nonexistent/moor.yaml"),
    ]);
    (dir, Moor::from_env(&env).expect("from_env"))
}

#[test]
fn version_is_ok_with_expected_keys() {
    let resp = moor().1.call(Request::Version {});
    assert_eq!(resp.class, ExitClass::Ok);
    assert_eq!(resp.cmd, "version");
    let value = resp.value.expect("value");
    let obj = value.as_object().expect("object");
    for key in ["version", "commit", "schema"] {
        assert!(obj.contains_key(key), "missing {key}");
    }
}

#[test]
fn unimplemented_command_fails_with_its_cmd() {
    let req = Request::Close(CloseReq {
        target: moor_core::name::BoxName::parse_allow_bare("t")
            .unwrap()
            .into(),
        force: false,
    });
    let (_dir, moor) = moor();
    let resp = moor.call(req);
    assert_eq!(resp.class, ExitClass::Failed);
    assert_eq!(resp.cmd, "close");
    assert!(resp.error.is_some());
}

#[test]
fn invalid_test_now_is_a_usage_error() {
    let env = Env::from_pairs([("MOOR_TEST_NOW", "yesterday-ish")]);
    let err = Moor::from_env(&env).err().expect("error");
    assert_eq!(err.kind, moor_core::ErrorKind::Usage);
    assert!(err.error.to_string().contains("MOOR_TEST_NOW"));
}

#[test]
fn only_a_front_socket_gives_an_added_workflow_its_origin_front() {
    use moor_core::request::{UserCmd, WorkflowAddReq, WorkflowCmd};
    use moor_core::user::Caller;

    let req = WorkflowAddReq {
        identity: None,
        name: "n".to_owned(),
        cron: "0 9 * * *".to_owned(),
        timezone: "UTC".to_owned(),
        repos: vec!["o/n".to_owned()],
        prompt: "p".to_owned(),
        timeout_minutes: None,
        origin_key: Some("k".to_owned()),
    };
    let add = UserCmd::Workflow(WorkflowCmd::Add {
        req: req.clone(),
        origin_front: Some("forged".to_owned()),
    });
    let with = |front: Option<&str>| {
        UserCmd::Workflow(WorkflowCmd::Add {
            req: req.clone(),
            origin_front: front.map(str::to_owned),
        })
    };
    let got = |c: Caller| super::with_origin(&add, &Ok(c));
    assert_eq!(got(Caller::Front("slack".to_owned())), with(Some("slack")));
    assert_eq!(got(Caller::Operator(1000)), with(None));
    assert_eq!(got(Caller::Local(1000)), with(None));
}
