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

#[test]
fn a_runner_listening_beyond_loopback_without_a_service_key_refuses_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_moor-machine-runner"))
        .env_remove("MOOR_MACHINE_SERVICE_KEY")
        .args(["--listen", "0.0.0.0:0", "--socket"])
        .arg(dir.path().join("runner.sock"))
        .arg("--state-dir")
        .arg(dir.path().join("state"))
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("needs --service-key"), "{err}");
}
