use std::path::PathBuf;

use super::*;

fn key(n: i64) -> BoxKey {
    BoxKey::new(BoxId::new(n))
}

fn name(s: &str) -> BoxName {
    BoxName::parse_allow_bare(s).unwrap()
}

#[test]
fn slug_carries_id_and_nonce() {
    assert_eq!(key(7).slug("a1b2c3"), "moor-7-a1b2c3");
}

#[test]
fn id_from_slug_round_trips() {
    assert_eq!(BoxKey::id_from_slug("moor-7-a1b2c3"), Some(BoxId::new(7)));
    assert_eq!(
        BoxKey::id_from_slug(&key(i64::MAX).slug("0f")),
        Some(BoxId::new(i64::MAX))
    );
}

#[test]
fn id_from_slug_rejects_foreign_slugs() {
    for slug in [
        "moor-fix-bug-a1b2c3",
        "moor--abc",
        "moor-7",
        "moor-7-",
        "other-7-abc",
        "moor-x7-abc",
        "moor-7-xyz",
        "moor-7-ABC",
        "moor-7-ab-c",
        "moor-99999999999999999999-abc",
        "",
    ] {
        assert_eq!(BoxKey::id_from_slug(slug), None, "{slug}");
    }
}

#[test]
fn cred_and_tmp_dirs_carry_id_and_nonce() {
    assert_eq!(key(7).cred_dir("n"), ".moor-cred-7-n");
    assert_eq!(key(7).tmp_dir("n"), ".moor-tmp-7-n");
}

#[test]
fn proof_paths_are_under_the_id() {
    assert_eq!(key(7).proof_dir(), PathBuf::from("proof/7"));
    assert_eq!(
        key(7).proof_file("sub/b.txt"),
        PathBuf::from("proof/7/sub/b.txt")
    );
}

#[test]
fn branch_flattens_slashes() {
    assert_eq!(branch(&name("moor/7")), "moor/moor-7");
    assert_eq!(branch(&name("web/fix-bug")), "moor/web-fix-bug");
    assert_eq!(branch(&name("fix-bug")), "moor/fix-bug");
}

#[test]
fn branch_holds_exactly_one_slash() {
    for n in ["moor/7", "web/fix-bug", "fix-bug"] {
        assert_eq!(branch(&name(n)).matches('/').count(), 1, "{n}");
    }
}

#[test]
fn branch_on_collision_appends_id() {
    assert_eq!(
        key(7).branch_on_collision(&name("web/fix-bug")),
        "moor/web-fix-bug-7"
    );
}

#[test]
fn slug_fits_crabbox_cap_for_any_id() {
    for id in [0, 1, 7, 123_456_789, i64::MAX] {
        let slug = key(id).slug("a1b2c3");
        assert!(slug.len() <= 41, "{slug}");
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{slug}"
        );
    }
}

#[test]
fn session_ref_carries_the_id() {
    assert_eq!(BoxKey::new(BoxId::new(7)).session_ref(), "moor-7");
}
