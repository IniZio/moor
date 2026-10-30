use std::os::unix::fs::symlink;
use std::path::Path;

use moor_core::ErrorKind;

use super::inside;

struct Fx {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    outside: std::path::PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().join("runner");
    let outside = tmp.path().join("controller");
    std::fs::create_dir_all(root.join("pulls")).expect("root");
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::write(root.join("pulls/1-0"), b"x").expect("staged");
    std::fs::write(outside.join("vault.key"), b"secret").expect("key");
    Fx {
        _tmp: tmp,
        root,
        outside,
    }
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn refused(path: &Path, root: &Path, says: &str) {
    let e = inside("the destination", &s(path), root).expect_err("refused");
    assert_eq!(e.kind, ErrorKind::Refused, "{}", e.error);
    assert!(e.error.to_string().contains(says), "{}", e.error);
}

#[test]
fn a_path_inside_the_root_passes_resolved_whether_or_not_it_exists() {
    let f = fx();
    let real = f.root.canonicalize().expect("real");
    let file = inside("the source", &s(&f.root.join("pulls/1-0")), &f.root).expect("file");
    assert_eq!(file, real.join("pulls/1-0"));
    let fresh = inside("the destination", &s(&f.root.join("mirrors/7")), &f.root).expect("new");
    assert_eq!(fresh, real.join("mirrors/7"));
}

#[test]
fn a_root_behind_a_symlink_still_holds_its_own_paths() {
    let f = fx();
    let link = f.outside.join("runner-link");
    symlink(&f.root, &link).expect("link");
    inside("the destination", &s(&link.join("mirrors/7")), &link).expect("inside");
}

#[test]
fn a_path_outside_the_root_is_refused() {
    let f = fx();
    refused(&f.outside.join("vault.key"), &f.root, "is outside");
    refused(Path::new("/etc"), &f.root, "is outside");
}

#[test]
fn a_parent_component_is_refused_even_when_it_lands_inside() {
    let f = fx();
    refused(&f.root.join("pulls/../pulls/1-0"), &f.root, "`..`");
    refused(&f.root.join("../controller/vault.key"), &f.root, "`..`");
}

#[test]
fn a_symlinked_directory_leading_out_is_refused() {
    let f = fx();
    symlink(&f.outside, f.root.join("mirrors")).expect("link");
    refused(&f.root.join("mirrors/7"), &f.root, "is outside");
    refused(&f.root.join("mirrors/vault.key"), &f.root, "is outside");
}

#[test]
fn a_path_that_is_itself_a_symlink_out_is_refused() {
    let f = fx();
    symlink(f.outside.join("vault.key"), f.root.join("pulls/2-0")).expect("file link");
    refused(&f.root.join("pulls/2-0"), &f.root, "is outside");
    symlink(&f.outside, f.root.join("dest")).expect("dir link");
    refused(&f.root.join("dest"), &f.root, "is outside");
}

#[test]
fn a_dangling_symlink_is_refused() {
    let f = fx();
    symlink(f.outside.join("not-yet"), f.root.join("dangling")).expect("link");
    refused(&f.root.join("dangling"), &f.root, "cannot resolve");
}

#[test]
fn a_missing_root_refuses_everything() {
    let f = fx();
    refused(&f.root.join("x"), &f.outside.join("nope"), "cannot resolve");
}

