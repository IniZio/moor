//! Tests for identities, front namespaces and secret names.

use super::{valid_front_namespace, Identity, SecretName};

#[test]
fn an_identity_splits_at_the_first_colon_only() {
    let id = Identity::parse("slack:T1:U2").unwrap();
    assert_eq!(id.namespace(), "slack");
    assert_eq!(id.as_str(), "slack:T1:U2");
    assert_eq!(Identity::local(1000).as_str(), "local:1000");
}

#[test]
fn malformed_identities_are_refused() {
    let long = format!("slack:{}", "x".repeat(251));
    for bad in [
        "",
        "slack",
        ":u",
        "slack:",
        "Slack:u",
        "1x:u",
        "a_b:u",
        long.as_str(),
    ] {
        assert!(Identity::parse(bad).is_err(), "{bad:?} parsed");
    }
}

#[test]
fn identities_deserialize_through_the_parser() {
    let ok: Identity = serde_json::from_str("\"web:alice\"").unwrap();
    assert_eq!(ok.namespace(), "web");
    assert!(serde_json::from_str::<Identity>("\"alice\"").is_err());
}

#[test]
fn local_is_reserved_for_the_controller_socket() {
    assert!(valid_front_namespace("slack"));
    assert!(valid_front_namespace("chat-2"));
    for bad in ["local", "", "Slack", "2chat", "a:b", "a_b"] {
        assert!(!valid_front_namespace(bad), "{bad:?} accepted");
    }
}

#[test]
fn secret_names_are_lower_words() {
    assert!(SecretName::parse("claude").is_ok());
    assert!(SecretName::parse("github_token-2").is_ok());
    for bad in ["", "Claude", "1x", "a b", "a/b", &"x".repeat(65)] {
        assert!(SecretName::parse(bad).is_err(), "{bad:?} parsed");
    }
}
