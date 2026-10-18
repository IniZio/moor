//! Tests for name validation.

use super::{BoxHandle, BoxId, BoxName, BoxPath, SessionId, Sha, TaskId, TaskName};

/// Valid Box names parse.
#[test]
fn box_name_valid() {
    assert_eq!(BoxName::parse("a").unwrap().as_str(), "a");
    assert!(BoxName::parse("fix-bug-9").is_ok());
    assert!(BoxName::parse("9lives").is_ok());
    assert!(BoxName::parse(&"a".repeat(40)).is_ok());
}

/// Invalid Box names are rejected.
#[test]
fn box_name_invalid() {
    assert!(BoxName::parse("").is_err());
    assert!(BoxName::parse("Abc").is_err());
    assert!(BoxName::parse("-abc").is_err());
    assert!(BoxName::parse("a_b").is_err());
    assert!(BoxName::parse(&"a".repeat(41)).is_err());
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
fn box_name_wire_matches_task_name() {
    let name = BoxName::parse("fix-bug-9").unwrap();
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

/// The old Task names are aliases of the Box types.
#[test]
fn task_aliases_are_box_types() {
    let name: TaskName = BoxName::parse("a").unwrap();
    let _: BoxName = name;
    assert_eq!(TaskId::new(3), BoxId::new(3));
}
