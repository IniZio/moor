//! A rules text: a list of rules with no names.

use crate::config::proxy::{parse_rules_text, DEFAULT_RULES_TEXT};

const NPM: &str = "rules:
  - hosts: [npm.pkg.github.com, npm.example.com]
    credential: npm-token
    inject: {form: 'Bearer {secret}'}
";

#[test]
fn the_default_text_has_no_rule_names() {
    let first_level: Vec<&str> = DEFAULT_RULES_TEXT
        .lines()
        .filter(|l| !l.starts_with(' ') && !l.starts_with('#') && !l.is_empty())
        .collect();
    assert_eq!(first_level, ["rules:"]);
    for name in ["claude:", "github-api", "github-git", "github-uploads"] {
        assert!(!DEFAULT_RULES_TEXT.contains(name), "{name}");
    }
}

#[test]
fn a_rule_is_known_by_its_first_host() {
    let rules = parse_rules_text("t", NPM).unwrap();
    assert_eq!(rules.keys().collect::<Vec<_>>(), ["npm.pkg.github.com"]);
}

#[test]
fn the_default_hosts_keep_their_ids() {
    let text = "rules:
  - hosts: [api.github.com]
    credential: github
    inject: {form: 'token {secret}'}
  - hosts: [github.com]
    credential: github
    inject: {basic_user: x-access-token}
";
    let rules = parse_rules_text("t", text).unwrap();
    assert_eq!(
        rules.keys().collect::<Vec<_>>(),
        ["github-api", "github-git"]
    );
}

#[test]
fn a_host_belongs_to_one_rule() {
    let text = format!(
        "{NPM}  - hosts: [other.example.com, NPM.example.com]
    credential: other
    inject: {{form: 'Bearer {{secret}}'}}
"
    );
    let e = parse_rules_text("t", &text).unwrap_err();
    assert_eq!(e.path, "rules[1].hosts[1]");
    assert!(e.reason.contains("also listed by rules[0]"), "{e}");
}

#[test]
fn an_error_names_the_rule_by_its_place() {
    let e = parse_rules_text(
        "t",
        "rules:\n  - hosts: [a.example.com]\n    credential: NOPE\n    inject: {basic_user: u}\n",
    )
    .unwrap_err();
    assert_eq!(e.path, "rules[0].credential");
}

#[test]
fn rule_names_are_refused() {
    let e = parse_rules_text("t", "github-api:\n  hosts: [api.github.com]\n").unwrap_err();
    assert!(e.reason.contains("unknown field"), "{e}");
}
