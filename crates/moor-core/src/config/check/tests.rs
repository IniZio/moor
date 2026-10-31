//! Tests for the shared validators.

use super::*;

#[test]
fn credential_names() {
    for s in ["a", "A-b_9"] {
        assert_eq!(credential_name(s), Ok(()), "{s:?}");
    }
    for s in ["", "a b", "a.b", "a/b"] {
        assert_eq!(
            credential_name(s),
            Err(format!("name {s:?} must match [A-Za-z0-9_-]+"))
        );
    }
}

#[test]
fn env_names() {
    for s in ["A", "_x", "A_1"] {
        assert_eq!(env_name(s), Ok(()), "{s:?}");
    }
    for s in ["", "1A", "A-B", "A B"] {
        assert_eq!(
            env_name(s),
            Err(format!("name {s:?} must match [A-Za-z_][A-Za-z0-9_]*"))
        );
    }
}

#[test]
fn reserved_env() {
    for s in ["path", "HOME", ""] {
        assert_eq!(not_reserved_env(s), Ok(()), "{s:?}");
    }
    assert_eq!(
        not_reserved_env("PATH"),
        Err(r#"name "PATH" is reserved"#.to_owned())
    );
}

#[test]
fn host_entries() {
    for s in ["Example.COM", "*.example.org", " a-b.io "] {
        assert_eq!(host_entry(s), Ok(()), "{s:?}");
    }
    for s in [
        "10.0.0.1",
        "localhost",
        "bad_char.com",
        "https://x.com",
        "",
        "*.",
        "::1",
    ] {
        assert_eq!(
            host_entry(s),
            Err(format!("host must be a domain or *.domain: {s:?}"))
        );
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn box_credentials_ok() {
    assert_eq!(
        box_credentials(&names(&["a", "b-1"]), &BTreeMap::new()),
        Ok(())
    );
    assert_eq!(box_credentials(&[], &BTreeMap::new()), Ok(()));
}

#[test]
fn box_credentials_bad_name() {
    assert_eq!(
        box_credentials(&names(&["a", "NPM_TOKEN"]), &BTreeMap::new()),
        Err((1, SecretName::parse("NPM_TOKEN").unwrap_err()))
    );
}

#[test]
fn box_credentials_duplicate() {
    assert_eq!(
        box_credentials(&names(&["a", "a"]), &BTreeMap::new()),
        Err((1, r#"box credential "a" is listed twice"#.to_owned()))
    );
}

#[test]
fn repo_env_may_not_set_the_sandbox_signal() {
    assert!(agent_env_reserved("IS_SANDBOX"));
    assert!(!agent_env_reserved("IS_SANDBOXED"));
}

#[test]
fn repo_env_may_not_redirect_git_config() {
    assert!(agent_env_reserved("XDG_CONFIG_HOME"));
    assert!(agent_env_reserved("GIT_CONFIG_COUNT"));
}

#[test]
fn repo_env_may_set_nono_and_other_xdg_names() {
    assert!(!agent_env_reserved("NONO_PROFILE"));
    assert!(!agent_env_reserved("XDG_DATA_HOME"));
}

#[test]
fn a_refused_agent_env_name_is_a_credential_or_one_moor_sets() {
    for name in [
        "PATH",
        "NODE_OPTIONS",
        "LD_PRELOAD",
        "DEPLOY_TOKEN",
        "api_key",
    ] {
        assert!(agent_env_refused(name), "{name}");
    }
    for name in ["BUN_OPTIONS", "APP_MODE"] {
        assert!(!agent_env_refused(name), "{name}");
    }
}
