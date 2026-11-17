use std::os::unix::fs::PermissionsExt as _;

use moor_core::config::EnrolledMachine;
use moor_core::view::node::NodeAddValue;

use super::{agent_dir, finish, prepare, run_lines, runner_url, Add};

fn add(home: &std::path::Path) -> Add {
    Add {
        name: "local".to_owned(),
        agent_home: home.join("agent"),
        runner: "127.0.0.1:7701".to_owned(),
        labels: vec!["linux".to_owned()],
        max_boxes: 3,
    }
}

#[test]
fn a_bare_loopback_address_is_plain_http_and_a_remote_one_needs_tls() {
    assert_eq!(
        runner_url("127.0.0.1:7701").unwrap(),
        "http://127.0.0.1:7701"
    );
    assert_eq!(
        runner_url("http://[::1]:7701/").unwrap(),
        "http://[::1]:7701"
    );
    assert_eq!(
        runner_url("localhost:7701").unwrap(),
        "http://localhost:7701"
    );
    assert_eq!(
        runner_url("https://runner.example.com").unwrap(),
        "https://runner.example.com"
    );
    for refused in [
        "10.0.0.5:7701",
        "http://runner.example.com:7701",
        "127.0.0.1",
        "ftp://127.0.0.1:7701",
        "",
    ] {
        assert!(runner_url(refused).is_err(), "{refused}");
    }
}

#[test]
fn preparing_makes_a_private_key_the_controller_accepts_and_keeps_it_on_a_rerun() {
    let dir = tempfile::tempdir().unwrap();
    let add = add(dir.path());

    let first = prepare(&add).unwrap();
    let again = prepare(&add).unwrap();

    assert_eq!(first, again, "a rerun enrolls the same key");
    assert_eq!(first.name, "local");
    assert_eq!(first.labels, ["linux"]);
    assert_eq!(first.capacity, 3);
    EnrolledMachine::parse(&first.name, &first.pubkey, &first.labels, first.capacity)
        .expect("the controller takes the key as written");
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let agent = agent_dir(&add.agent_home);
    assert_eq!(mode(&add.agent_home), 0o700);
    assert_eq!(mode(&agent), 0o700);
    assert_eq!(mode(&agent.join("id_ed25519")), 0o600);
}

#[test]
fn finishing_writes_the_state_an_agent_run_under_that_home_reads() {
    let dir = tempfile::tempdir().unwrap();
    let add = add(dir.path());
    prepare(&add).unwrap();
    let added = NodeAddValue {
        machine_id: 12,
        name: "local".to_owned(),
    };

    finish(&add, &added).unwrap();

    let path = agent_dir(&add.agent_home).join("state.json");
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        state,
        serde_json::json!({
            "server": "http://127.0.0.1:7701",
            "machine_id": 12,
            "owner": "operator",
            "name": "local",
            "slots": 3,
        })
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(run_lines(&add).contains("moor-machine-agent run"));
}

#[test]
fn a_refused_runner_makes_no_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut add = add(dir.path());
    add.runner = "10.0.0.5:7701".to_owned();

    assert!(prepare(&add).is_err());
    assert!(!add.agent_home.exists());
}
