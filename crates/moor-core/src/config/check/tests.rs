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
fn gh_token() {
    assert_eq!(not_gh_token("GH_TOKEN_X"), Ok(()));
    assert_eq!(not_gh_token("gh_token"), Ok(()));
    assert_eq!(
        not_gh_token("GH_TOKEN"),
        Err(
            "GH_TOKEN is not supported: moor mints GitHub tokens from your GitHub App; \
              run `moor github setup` then `moor github login`"
                .to_owned()
        )
    );
}

#[test]
fn proxied_claude_vars() {
    for s in [
        "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
    ] {
        assert!(proxied_claude_var(s), "{s:?}");
    }
    for s in ["CLAUDE_X", "MY_ANTHROPIC_KEY", "GH_TOKEN"] {
        assert!(!proxied_claude_var(s), "{s:?}");
        assert_eq!(not_proxied_claude_var(s), Ok(()), "{s:?}");
    }
    assert_eq!(
        not_proxied_claude_var("ANTHROPIC_API_KEY"),
        Err(
            "ANTHROPIC_API_KEY is not supported: the moor proxy supplies Claude credentials"
                .to_owned()
        )
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
    assert_eq!(box_credentials(&names(&["a", "b-1"])), Ok(()));
    assert_eq!(box_credentials(&[]), Ok(()));
}

#[test]
fn box_credentials_bad_name() {
    assert_eq!(
        box_credentials(&names(&["a", "b c"])),
        Err((1, credential_name("b c").unwrap_err()))
    );
}

#[test]
fn box_credentials_rejects_gh_token() {
    assert_eq!(
        box_credentials(&names(&["GH_TOKEN"])),
        Err((0, not_gh_token("GH_TOKEN").unwrap_err()))
    );
}

#[test]
fn box_credentials_rejects_proxied_claude() {
    for s in ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"] {
        assert_eq!(
            box_credentials(&names(&["ok", s])),
            Err((
                1,
                format!("{s} is not supported: the moor proxy supplies Claude credentials")
            ))
        );
    }
}

#[test]
fn box_credentials_duplicate() {
    assert_eq!(
        box_credentials(&names(&["a", "a"])),
        Err((1, r#"box credential "a" is listed twice"#.to_owned()))
    );
}
