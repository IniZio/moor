use super::super::ForgeRepo;
use super::ProfileSource;

#[test]
fn a_github_url_parses_with_or_without_dot_git_and_ref() {
    for (text, git_ref, shown) in [
        (
            "https://github.com/alice/agent",
            None,
            "https://github.com/alice/agent",
        ),
        (
            "https://github.com/alice/agent.git",
            None,
            "https://github.com/alice/agent",
        ),
        (
            "https://github.com/alice/agent.git#v2",
            Some("v2"),
            "https://github.com/alice/agent#v2",
        ),
        (
            "https://github.com/alice/agent#feature/x",
            Some("feature/x"),
            "https://github.com/alice/agent#feature/x",
        ),
    ] {
        let source = ProfileSource::parse(text).unwrap();
        assert_eq!(source.repo, ForgeRepo::new("alice", "agent"), "{text}");
        assert_eq!(source.git_ref.as_deref(), git_ref, "{text}");
        assert_eq!(source.to_string(), shown, "{text}");
    }
}

#[test]
fn anything_but_an_https_github_repo_is_refused() {
    for text in [
        "https://gitlab.com/alice/agent",
        "http://github.com/alice/agent",
        "ssh://git@github.com/alice/agent",
        "git@github.com:alice/agent",
        "https://token@github.com/alice/agent",
        "https://github.com/alice",
        "https://github.com/",
        "https://github.com/alice/agent/extra",
        "https://github.com/alice/agent#",
        "https://github.com/alice/agent#-x",
        "https://github.com/alice/agent#a b",
        "alice/agent",
        "",
    ] {
        assert!(ProfileSource::parse(text).is_err(), "{text}");
    }
}

#[test]
fn the_wire_form_is_the_url_text() {
    let source = ProfileSource::parse("https://github.com/alice/agent#v2").unwrap();
    let wire = serde_json::to_value(&source).unwrap();
    assert_eq!(wire, serde_json::json!("https://github.com/alice/agent#v2"));
    assert_eq!(
        serde_json::from_value::<ProfileSource>(wire).unwrap(),
        source
    );
    assert!(
        serde_json::from_value::<ProfileSource>(serde_json::json!("https://example.com/a/b"))
            .is_err()
    );
}
