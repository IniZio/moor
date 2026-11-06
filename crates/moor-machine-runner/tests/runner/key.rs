use serde_json::json;

use crate::rig::{Rig, HEADERS};

const KEY: &str = "X-Moor-Service-Key";

fn with(key: Option<&str>) -> Vec<(&str, &str)> {
    let mut headers = HEADERS.to_vec();
    headers.extend(key.map(|k| (KEY, k)));
    headers
}

#[test]
fn a_runner_with_a_service_key_names_a_box_only_for_a_caller_sending_it() {
    let rig = Rig::with_args(&["--service-key", "the-key"]);
    for sent in [None, Some("wrong-key"), Some("the-ke"), Some("")] {
        let err = rig
            .call_with("machine_info", json!({}), &with(sent))
            .unwrap_err();
        assert!(err.contains(KEY), "{sent:?}: {err}");
    }
    let info = rig
        .call_with("machine_info", json!({}), &with(Some("the-key")))
        .unwrap();
    assert_eq!(info["box"], 7, "{info}");
}

#[test]
fn a_runner_without_a_service_key_trusts_the_headers_alone() {
    let rig = Rig::start();
    assert!(rig
        .call_with("machine_info", json!({}), &with(None))
        .is_ok());
}
