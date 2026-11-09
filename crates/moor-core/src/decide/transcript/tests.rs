use super::*;

#[test]
fn path_names_box_session_and_digest() {
    assert_eq!(
        relative_path(BoxId::new(4), 9, "ab12"),
        "transcripts/4/9/ab12.tgz"
    );
}

#[test]
fn digest_is_sha256_hex() {
    assert_eq!(
        digest(&b"abc"[..]).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn triggers_are_named() {
    assert_eq!(Trigger::Relaunch.name(), "relaunch");
    assert_eq!(Trigger::Close.name(), "close");
    assert_eq!(Trigger::End.name(), "end");
    assert_eq!(Trigger::Exit.name(), "exit");
}

#[test]
fn an_archive_repeats_only_an_equal_stored_digest() {
    assert!(is_duplicate(Some("a"), "a"));
    assert!(!is_duplicate(Some("a"), "b"));
    assert!(!is_duplicate(None, "a"));
}
