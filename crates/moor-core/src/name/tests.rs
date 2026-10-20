//! Tests for name validation.

use super::{next_box_name, BoxHandle, BoxId, BoxName, BoxPath, SessionId, Sha};

/// Valid Box names parse.
#[test]
fn box_name_valid() {
    assert_eq!(BoxName::parse_allow_bare("a").unwrap().as_str(), "a");
    assert!(BoxName::parse_allow_bare("fix-bug-9").is_ok());
    assert!(BoxName::parse_allow_bare("9lives").is_ok());
    assert!(BoxName::parse_allow_bare(&"a".repeat(40)).is_ok());
}

/// Invalid Box names are rejected.
#[test]
fn box_name_invalid() {
    assert!(BoxName::parse_allow_bare("").is_err());
    assert!(BoxName::parse_allow_bare("Abc").is_err());
    assert!(BoxName::parse_allow_bare("-abc").is_err());
    assert!(BoxName::parse_allow_bare("a_b").is_err());
    assert!(BoxName::parse_allow_bare(&"a".repeat(41)).is_err());
}

/// Sha length and charset limits hold.
#[test]
fn sha_rules() {
    assert!(Sha::parse("abc").is_err());
    assert!(Sha::parse("abcd").is_ok());
    assert!(Sha::parse(&"a".repeat(40)).is_ok());
    assert!(Sha::parse(&"a".repeat(41)).is_err());
    assert!(Sha::parse("abcg").is_err());
    assert_eq!(Sha::parse("ABCD1234").unwrap().as_str(), "abcd1234");
}

/// `BoxPath::under` checks both parts and joins with `/`.
#[test]
fn box_path_under() {
    assert!(BoxPath::under("work", "a").is_err());
    assert!(BoxPath::under("/w", "../a").is_err());
    assert!(BoxPath::under("/w", "a/../b").is_err());
    assert!(BoxPath::under("/w", "/a").is_err());
    assert!(BoxPath::under("/w", "").is_err());
    assert_eq!(BoxPath::under("/w", "a/b").unwrap().as_str(), "/w/a/b");
    assert_eq!(BoxPath::under("/w/", "a").unwrap().as_str(), "/w/a");
}

/// `BoxPath::restore` keeps absolute text unchanged, `..` included.
#[test]
fn box_path_restore() {
    let kept = BoxPath::restore("/p/bin/../box/work/box-1").unwrap();
    assert_eq!(kept.as_str(), "/p/bin/../box/work/box-1");
    assert!(BoxPath::restore("").is_err());
    assert!(BoxPath::restore("work/box-1").is_err());
    assert!(BoxPath::restore("/").is_err());
}

/// `BoxId` is a bare integer and `BoxHandle` a bare string in JSON.
#[test]
fn box_id_and_box_handle_serde_shape() {
    let id = BoxId::new(7);
    assert_eq!(serde_json::to_string(&id).unwrap(), "7");
    assert_eq!(serde_json::from_str::<BoxId>("7").unwrap(), id);
    let h = BoxHandle::new("box-1");
    assert_eq!(serde_json::to_string(&h).unwrap(), "\"box-1\"");
    assert_eq!(serde_json::from_str::<BoxHandle>("\"box-1\"").unwrap(), h);
}

/// A Box name is a bare JSON string and still validates on the way in.
#[test]
fn box_name_wire_matches_box_name() {
    let name = BoxName::parse_allow_bare("fix-bug-9").unwrap();
    assert_eq!(serde_json::to_string(&name).unwrap(), "\"fix-bug-9\"");
    assert_eq!(
        serde_json::from_str::<BoxName>("\"fix-bug-9\"").unwrap(),
        name
    );
    assert!(serde_json::from_str::<BoxName>("\"Bad\"").is_err());
}

/// A Box id is a bare JSON integer.
#[test]
fn box_id_wire_is_bare_integer() {
    let id = BoxId::new(7);
    assert_eq!(serde_json::to_string(&id).unwrap(), "7");
    assert_eq!(serde_json::from_str::<BoxId>("7").unwrap(), id);
}

/// A Session id is a bare JSON integer.
#[test]
fn session_id_wire_is_bare_integer() {
    let id = SessionId::new(7);
    assert_eq!(serde_json::to_string(&id).unwrap(), "7");
    assert_eq!(serde_json::from_str::<SessionId>("7").unwrap(), id);
    assert_eq!(id.get(), 7);
}

/// Strict names are `<repo>/<label>`.
#[test]
fn parse_accepts_repo_slash_label() {
    assert_eq!(BoxName::parse("moor/7").unwrap().as_str(), "moor/7");
    assert!(BoxName::parse("moor/fix-bug").is_ok());
    assert!(BoxName::parse(&format!("moor/{}", "a".repeat(40))).is_ok());
    assert!(BoxName::parse(&format!("{}/x", "a".repeat(40))).is_ok());
}

/// Strict parse rejects malformed and bare names.
#[test]
fn parse_rejects_malformed() {
    for bad in [
        "/a", "a/", "a//b", "a/b/c", "A/b", "-a/b", "a/-b", "fix-bug", "", "/",
    ] {
        assert!(BoxName::parse(bad).is_err(), "{bad}");
    }
    assert!(BoxName::parse(&format!("a/{}", "b".repeat(41))).is_err());
    assert!(BoxName::parse(&format!("{}/b", "a".repeat(41))).is_err());
}

/// `parse_allow_bare` takes strict names and bare segments only.
#[test]
fn parse_allow_bare_accepts_bare_and_strict() {
    assert!(BoxName::parse_allow_bare("fix-bug").is_ok());
    assert!(BoxName::parse_allow_bare("slack-1700000000-000100").is_ok());
    assert!(BoxName::parse_allow_bare("moor/7").is_ok());
    assert!(BoxName::parse_allow_bare("a/b/c").is_err());
    assert!(BoxName::parse_allow_bare("a/").is_err());
}

/// `repo` and `label` split a name.
#[test]
fn repo_and_label() {
    let n = BoxName::parse("moor/fix").unwrap();
    assert_eq!((n.repo(), n.label()), (Some("moor"), "fix"));
    let b = BoxName::parse_allow_bare("fix").unwrap();
    assert_eq!((b.repo(), b.label()), (None, "fix"));
}

/// The counter starts at 1.
#[test]
fn next_box_name_empty_starts_at_one() {
    assert_eq!(next_box_name("moor", &[]).unwrap().as_str(), "moor/1");
}

/// The counter goes above the high-water mark and never fills gaps.
#[test]
fn next_box_name_uses_max_not_gap() {
    assert_eq!(
        next_box_name("moor", &[1, 2, 5]).unwrap().as_str(),
        "moor/6"
    );
    assert_eq!(next_box_name("moor", &[9, 3]).unwrap().as_str(), "moor/10");
}

/// A closed Box's high n is still passed in, so it is not reused.
#[test]
fn next_box_name_stays_above_closed_high_n() {
    assert_eq!(next_box_name("moor", &[1, 40]).unwrap().as_str(), "moor/41");
}

/// An invalid repo is an error.
#[test]
fn next_box_name_rejects_invalid_repo() {
    assert!(next_box_name("Bad Repo", &[]).is_err());
    assert!(next_box_name("a/b", &[1]).is_err());
}

/// A `--` inside either segment of a strict name is rejected.
#[test]
fn parse_rejects_double_dash() {
    for bad in ["a--b/c", "a/b--c"] {
        assert!(BoxName::parse(bad).is_err(), "{bad}");
        assert!(BoxName::parse_allow_bare(bad).is_err(), "{bad}");
    }
}

/// A bare name may still hold `--`, because the store may hold such names.
#[test]
fn parse_allow_bare_keeps_double_dash() {
    assert!(BoxName::parse_allow_bare("a--b").is_ok());
    assert!(BoxName::parse("a--b").is_err());
}
