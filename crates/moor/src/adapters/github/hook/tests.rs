use super::HookSecret;

/// GitHub's own example: secret "It's a Secret to Everybody", body "Hello, World!".
const GITHUB_EXAMPLE: &str =
    "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";

#[test]
fn the_signature_github_documents_matches() {
    let secret = HookSecret::new(b"It's a Secret to Everybody").unwrap();
    assert!(secret.signed(b"Hello, World!", Some(GITHUB_EXAMPLE)));
}

#[test]
fn a_wrong_missing_or_other_body_signature_does_not_match() {
    let secret = HookSecret::new(b"It's a Secret to Everybody").unwrap();
    assert!(!secret.signed(b"Hello, World!", None));
    assert!(!secret.signed(b"Hello, World!", Some("")));
    assert!(!secret.signed(b"Hello, World?", Some(GITHUB_EXAMPLE)));
    let upper = GITHUB_EXAMPLE.to_ascii_uppercase();
    assert!(!secret.signed(b"Hello, World!", Some(&upper)));
    let other = HookSecret::new(b"another").unwrap();
    assert!(!other.signed(b"Hello, World!", Some(GITHUB_EXAMPLE)));
}

#[test]
fn an_empty_secret_is_no_secret() {
    assert!(HookSecret::new(b"").is_none());
}

#[test]
fn load_trims_the_secret_file_and_refuses_an_empty_or_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret");
    let same = |p: &std::path::Path| p.to_path_buf();
    std::fs::write(&path, "  \n").unwrap();
    let err = HookSecret::load(&path, same).err().unwrap();
    assert!(err.contains("is empty"), "{err}");
    std::fs::write(&path, "It's a Secret to Everybody\n").unwrap();
    let secret = HookSecret::load(&path, same).unwrap();
    assert!(secret.signed(b"Hello, World!", Some(GITHUB_EXAMPLE)));
    let err = HookSecret::load(&dir.path().join("gone"), same)
        .err()
        .unwrap();
    assert!(err.contains("gone") && !err.contains("Secret"), "{err}");
}
