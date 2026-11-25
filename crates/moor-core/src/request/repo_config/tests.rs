//! The `repo-config-*` requests on the wire, and the requests older clients still send.

use serde_json::json;

use super::{RepoConfigGetReq, RepoConfigSetReq, RepoFile};
use crate::name::ForgeRepo;
use crate::request::Request;

fn set(expected_revision: Option<u64>) -> Request {
    Request::RepoConfigSet(RepoConfigSetReq {
        file: RepoFile::Config,
        identity: None,
        repo: ForgeRepo::new("o", "n"),
        text: "setup: [make]\n".into(),
        expected_revision,
    })
}

#[test]
fn the_rules_file_is_said_and_the_config_is_left_unsaid() {
    let rules: Request =
        serde_json::from_str(r#"{"cmd":"repo-config-get","repo":"o/n","file":"rules"}"#).unwrap();
    let Request::RepoConfigGet(get) = &rules else {
        unreachable!("{rules:?}")
    };
    assert_eq!(get.file, super::RepoFile::Rules);
    assert_eq!(
        serde_json::to_value(&rules).unwrap(),
        json!({"cmd":"repo-config-get","repo":"o/n","file":"rules"})
    );
}

#[test]
fn a_get_is_a_repo_and_nothing_else() {
    let get: Request = serde_json::from_str(r#"{"cmd":"repo-config-get","repo":"o/n"}"#).unwrap();
    assert_eq!(
        get,
        Request::RepoConfigGet(RepoConfigGetReq {
            file: RepoFile::Config,
            identity: None,
            repo: ForgeRepo::new("o", "n"),
        })
    );
    assert_eq!(get.cmd(), "repo-config-get");
    let extra = r#"{"cmd":"repo-config-get","repo":"o/n","bogus":1}"#;
    assert!(serde_json::from_str::<Request>(extra).is_err());
}

#[test]
fn a_set_leaves_an_unset_revision_off_the_wire_and_keeps_a_zero() {
    let none = set(None);
    let wire = serde_json::to_value(&none).unwrap();
    assert_eq!(
        wire,
        json!({"cmd":"repo-config-set","repo":"o/n","text":"setup: [make]\n"})
    );
    assert_eq!(serde_json::from_value::<Request>(wire).unwrap(), none);
    let zero = set(Some(0));
    let wire = serde_json::to_value(&zero).unwrap();
    assert_eq!(wire.get("expected_revision"), Some(&json!(0)));
    assert_eq!(serde_json::from_value::<Request>(wire).unwrap(), zero);
}

#[test]
fn a_start_as_the_last_release_sent_it_still_parses() {
    let old = r#"{"cmd":"start","name":"a","prompt":"p","issue":"1","base":"main","model":"m","node":"n","no_agent":false,"cold":true,"resume":false,"repo":"o/n","also":["o/m"]}"#;
    assert!(matches!(
        serde_json::from_str::<Request>(old).unwrap(),
        Request::Start(_)
    ));
}
