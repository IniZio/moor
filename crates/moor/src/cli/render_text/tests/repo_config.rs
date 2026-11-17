//! `repo config get` prints the YAML so it can go back to `set`; `set` says the new revision.

use moor_core::Response;
use serde_json::json;

use super::render;

#[test]
fn get_prints_the_stored_yaml_and_nothing_else() {
    let v = json!({"repo": "o/n", "text": "setup: [make]\n", "revision": 3,
        "edited_by": "local:1000", "edited_at": "2026-11-17T00:00:00Z"});
    let (out, err) = render(&Response::ok("repo-config-get", Some(v)), false);
    assert_eq!((out.as_str(), err.as_str()), ("setup: [make]\n", ""));
}

#[test]
fn get_of_a_repo_with_no_config_prints_a_yaml_comment() {
    let v = json!({"repo": "o/n", "text": null, "revision": 0});
    let (out, _) = render(&Response::ok("repo-config-get", Some(v)), false);
    assert_eq!(out, "# no config is stored for o/n\n");
}

#[test]
fn set_names_the_repo_and_its_new_revision() {
    let v = json!({"repo": "o/n", "text": "setup: [make]\n", "revision": 4,
        "edited_by": "local:1000", "edited_at": "2026-11-17T00:00:00Z"});
    let (out, _) = render(&Response::ok("repo-config-set", Some(v)), false);
    assert_eq!(out, "o/n config is now revision 4\n");
}
