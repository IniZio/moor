use super::{machine, serves, source};

#[test]
fn routes_are_the_settings_ones() {
    assert!(serves("POST", "/account/api/machines/token"));
    assert!(serves("DELETE", "/account/api/machines/laptop"));
    assert!(serves("GET", "/account/api/profile"));
    assert!(serves("PUT", "/account/api/profile"));
    assert!(serves("POST", "/account/api/profile/check"));
    assert!(serves("DELETE", "/account/api/profile"));
    assert!(serves("GET", "/account/api/github-app"));
}

#[test]
fn other_routes_are_not_settings() {
    assert!(!serves("GET", "/account/api/machines/token"));
    assert!(!serves("DELETE", "/account/api/machines/token"));
    assert!(!serves("DELETE", "/account/api/machines/a/b"));
    assert!(!serves("PUT", "/account/api/nodes/laptop"));
}

#[test]
fn a_machine_route_names_one_machine() {
    assert_eq!(machine("/account/api/machines/laptop"), Some("laptop"));
    assert_eq!(machine("/account/api/machines/"), None);
    assert_eq!(machine("/account/api/machines/token"), None);
}

#[test]
fn a_profile_write_carries_a_github_repo() {
    let ok = source(Some(
        br#"{"source": " https://github.com/acme/profile#main "}"#,
    ))
    .unwrap();
    assert_eq!(ok.to_string(), "https://github.com/acme/profile#main");
    assert!(source(Some(br#"{"source": "https://gitlab.com/a/b"}"#)).is_err());
    assert!(source(Some(b"{}")).is_err());
    assert!(source(None).is_err());
}
