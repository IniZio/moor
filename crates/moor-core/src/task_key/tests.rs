//! Tests for the Mac Box path key.

use super::{inside, refuse_inside_login_home, MacLayout, TaskKey};
use crate::name::{BoxPath, TaskName};

fn key(s: &str) -> TaskKey {
    TaskKey::new(&TaskName::parse(s).unwrap())
}

fn bp(s: &str) -> BoxPath {
    BoxPath::restore(s).unwrap()
}

/// Every layout path derives from the one key under the root.
#[test]
fn layout_paths_from_one_key() {
    let l = MacLayout::new(&bp("/Users/Shared/moor-n"), &key("t1")).unwrap();
    assert_eq!(l.root.as_str(), "/Users/Shared/moor-n/t1");
    for (got, dir) in [
        (&l.work, "work"),
        (&l.home, "home"),
        (&l.tmp, "tmp"),
        (&l.cache, "cache"),
        (&l.sim, "sim"),
        (&l.launch, "launch"),
    ] {
        assert_eq!(got.as_str(), format!("/Users/Shared/moor-n/t1/{dir}"));
    }
}

/// `find()` returns the layout for the work dir and anything below it.
#[test]
fn find_round_trips() {
    let k = key("t1");
    let l = MacLayout::new(&bp("/Users/Shared/moor-n"), &k).unwrap();
    assert_eq!(MacLayout::find(&l.work, &k), Some(l.clone()));
    let deep = bp("/Users/Shared/moor-n/t1/work/cbx_1/app");
    assert_eq!(MacLayout::find(&deep, &k), Some(l.clone()));
    let slash = bp("/Users/Shared/moor-n/t1/work/");
    assert_eq!(MacLayout::find(&slash, &k), Some(l));
}

/// `find()` rejects other keys, similar prefixes and non-work dirs.
#[test]
fn find_rejects() {
    let w = bp("/Users/Shared/moor-n/t10/work/x");
    assert_eq!(MacLayout::find(&w, &key("t1")), None);
    assert_eq!(MacLayout::find(&w, &key("t2")), None);
    assert!(MacLayout::find(&w, &key("t10")).is_some());
    assert_eq!(MacLayout::find(&bp("/a/xt1/work"), &key("t1")), None);
    assert_eq!(MacLayout::find(&bp("/a/t1/home/work"), &key("t1")), None);
    assert_eq!(MacLayout::find(&bp("/a/t1"), &key("t1")), None);
    assert_eq!(MacLayout::find(&bp("/a/t1/workx"), &key("t1")), None);
}

/// Credential dirs live under launch/ and tmp/.
#[test]
fn cred_paths() {
    let l = MacLayout::new(&bp("/r"), &key("t1")).unwrap();
    let n = "0123456789abcdef";
    assert_eq!(
        l.cred_dir(n).unwrap().as_str(),
        "/r/t1/launch/.moor-cred-0123456789abcdef"
    );
    assert_eq!(
        l.cred_tmp(n).unwrap().as_str(),
        "/r/t1/tmp/.moor-tmp-0123456789abcdef"
    );
    assert!(l.cred_dir("a/b").is_err());
    assert!(l.cred_tmp("").is_err());
}

/// A key that would escape the root is an error.
#[test]
fn escaping_key_is_error() {
    for bad in ["..", ".", "", "a/b", "../x"] {
        assert!(
            MacLayout::new(&bp("/r"), &TaskKey(bad.to_owned())).is_err(),
            "{bad}"
        );
    }
}

/// `inside()` is component-wise.
#[test]
fn inside_cases() {
    let home = bp("/Users/a");
    assert!(inside(&bp("/Users/a"), &home));
    assert!(inside(&bp("/Users/a/work/x"), &home));
    assert!(inside(&bp("/Users/a/"), &home));
    assert!(inside(&bp("/Users/a/x"), &bp("/Users/a/")));
    assert!(!inside(&bp("/Users/ab"), &home));
    assert!(!inside(&bp("/Users/ab/x"), &home));
    assert!(!inside(&bp("/Users"), &home));
}

/// `refuse_inside_login_home` names both paths.
#[test]
fn refuse_cases() {
    let home = bp("/Users/a");
    assert!(refuse_inside_login_home(&bp("/Users/Shared/w"), &home).is_ok());
    assert!(refuse_inside_login_home(&bp("/Users/ab/w"), &home).is_ok());
    let e = refuse_inside_login_home(&bp("/Users/a/w/"), &home).unwrap_err();
    assert!(
        e.contains("/Users/a/w/") && e.contains("/Users/a") && e.contains("outside the login home")
    );
}
