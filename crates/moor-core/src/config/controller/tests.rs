//! Controller config parse tables.
#![allow(clippy::panic, reason = "test assertions")]

use super::{parse_controller, ControllerConfig};

fn err(text: &str) -> crate::config::ConfigError {
    match parse_controller("c.yaml", text) {
        Ok(c) => panic!("accepted {text}: {c:?}"),
        Err(e) => e,
    }
}

#[test]
fn empty_documents_are_default() {
    for text in ["", "  \n", "null", "{}"] {
        let c = parse_controller("c.yaml", text).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(c, ControllerConfig::default(), "{text}");
    }
}

#[test]
fn both_keys_parse() {
    let c = parse_controller("c.yaml", "socket: ~/run/c.sock\nstate_dir: /var/moor\n")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.socket.as_deref(), Some("~/run/c.sock"));
    assert_eq!(c.state_dir.as_deref(), Some("/var/moor"));
}

#[test]
fn unknown_key_names_key_and_file() {
    let e = err("socket: /a\nbogus: 1\n");
    assert_eq!(e.file, "c.yaml");
    assert!(e.to_string().contains("bogus"), "{e}");
    assert!(e.to_string().contains("c.yaml"), "{e}");
}

#[test]
fn mistyped_value_names_key() {
    let e = err("socket: [a, b]\n");
    assert_eq!(e.path, "socket");
    assert_eq!(e.file, "c.yaml");
}

#[test]
fn empty_string_is_rejected() {
    for key in ["socket", "state_dir"] {
        let e = err(&format!("{key}: \"\"\n"));
        assert_eq!(e.path, key);
        assert_eq!(e.file, "c.yaml");
    }
}

#[test]
fn settle_interval_parses_and_defaults_to_five_seconds() {
    let c = parse_controller("c.yaml", "settle_every_secs: 30\n").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(c.settle_every(), std::time::Duration::from_secs(30));
    assert_eq!(
        ControllerConfig::default().settle_every(),
        std::time::Duration::from_secs(5)
    );
}

#[test]
fn zero_settle_interval_names_key() {
    let e = err("settle_every_secs: 0\n");
    assert_eq!(e.path, "settle_every_secs");
    assert_eq!(e.file, "c.yaml");
}
