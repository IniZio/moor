//! What a refusal reason keeps once redacted for an event.

use serde_json::{json, Value};

use super::{verdict, Verdict};

fn redacted(query: &str, vars: &Value) -> String {
    match verdict(query, vars) {
        Verdict::Deny(reason) => reason.redacted(),
        other => unreachable!("expected deny for {query}, got {other:?}"),
    }
}

#[test]
fn a_redacted_reason_keeps_names_and_hides_values() {
    let other = redacted(
        r#"{ repository(owner: "victim", name: "secrets") { id } }"#,
        &json!({}),
    );
    assert!(other.contains("field `repository`"), "{other}");
    assert!(
        !other.contains("victim") && !other.contains("secrets"),
        "{other}"
    );
    let field = redacted("{ viewer { email } }", &json!({}));
    assert!(field.contains("`email`"), "{field}");
    let search = redacted(
        r#"{ search(query: "repo:IniZio/moor password:hunter2", type: ISSUE, first: 1) { issueCount } }"#,
        &json!({}),
    );
    assert!(!search.contains("hunter2"), "{search}");
    let long = format!("{{ {} }}", "a".repeat(500));
    let capped = redacted(&long, &json!({}));
    assert!(capped.chars().count() <= 160, "{capped}");
    assert!(!capped.contains(&"a".repeat(65)), "{capped}");
}

#[test]
fn a_refused_search_keeps_its_shape_and_hides_its_values() {
    let query = r"query($q: String!) { search(query: $q, type: ISSUE, first: 1) { issueCount } }";
    let refused = |q: &str| match verdict(query, &json!({ "q": q })) {
        Verdict::Deny(reason) => reason.query().map(str::to_owned),
        other => unreachable!("expected deny for {q}, got {other:?}"),
    };
    assert_eq!(
        refused("( author:hunter2 ) repo:IniZio/moor state:open type:pr hunter2").as_deref(),
        Some("( author:… ) repo:{repo} state:open type:pr …"),
    );
    assert_eq!(
        refused("repo:victim/secrets OR org:victim").as_deref(),
        Some("repo:… OR org:…"),
    );
    let long = format!("repo:IniZio/moor {}", "label:x ".repeat(100));
    let capped = refused(&format!("{long} OR")).unwrap_or_default();
    assert!(capped.chars().count() <= 200, "{capped}");
    match verdict(r"{ viewer { email } }", &json!({})) {
        Verdict::Deny(reason) => assert_eq!(reason.query(), None),
        other => unreachable!("expected deny, got {other:?}"),
    }
}

#[test]
fn a_backtick_inside_a_value_cannot_unredact_it() {
    for (query, vars) in [
        (r#"{ "a`TERM`b" }"#, json!({})),
        (
            r"query($q: String!) { search(query: $q, type: ISSUE, first: 1) { issueCount } }",
            json!({"q": "repo:IniZio/moor a`TERM`b"}),
        ),
        (
            r#"query($o: String!) { repository(owner: $o, name: "moor") { id } }"#,
            json!({"o": "x`TERM`y"}),
        ),
    ] {
        let text = redacted(query, &vars);
        assert!(!text.contains("TERM"), "{query}: {text}");
    }
}
