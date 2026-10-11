use super::Moor;
use crate::env::Env;
use moor_core::{CloseReq, ExitClass, Request};

fn moor() -> Moor {
    let env = Env::from_pairs([("MOOR_STATE_DIR", "/tmp/x")]);
    Moor::from_env(&env).expect("from_env")
}

#[test]
fn version_is_ok_with_expected_keys() {
    let resp = moor().call(Request::Version {});
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
        task: "t".into(),
        force: false,
    });
    let resp = moor().call(req);
    assert_eq!(resp.class, ExitClass::Failed);
    assert_eq!(resp.cmd, "close");
    assert!(resp.error.is_some());
}
