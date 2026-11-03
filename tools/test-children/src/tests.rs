use super::*;

fn lines(src: &str) -> Vec<usize> {
    unguarded_spawns(&syn::parse_file(src).unwrap(), true)
}

#[test]
fn a_kill_shellout_in_test_code_is_flagged() {
    let src = "fn f() { Command::new(\"kill\").args([\"-HUP\", \"--\", &g]).status().unwrap(); }";
    assert_eq!(
        kill_shellouts(&syn::parse_file(src).unwrap(), true),
        vec![1]
    );
    assert!(kill_shellouts(&syn::parse_file(src).unwrap(), false).is_empty());
    let other = "fn f() { Command::new(\"sleep\").status().unwrap(); }";
    assert!(kill_shellouts(&syn::parse_file(other).unwrap(), true).is_empty());
}

#[test]
fn bare_child_binding_is_flagged() {
    assert_eq!(
        lines("fn f() { let c = Command::new(\"x\").spawn().unwrap(); }"),
        vec![1]
    );
}

#[test]
fn guard_wrapped_spawn_passes() {
    assert!(
        lines("fn f() { let c = ChildGuard::new(Command::new(\"x\").spawn().unwrap()); }")
            .is_empty()
    );
    assert!(lines("fn f() { let c = moor_testkit::ChildGuard::new(cmd.spawn()?); }").is_empty());
    assert!(lines("fn f() { let c = cmd.spawn().map(ChildGuard::new); }").is_empty());
}

#[test]
fn a_binding_wrapped_later_in_the_block_passes() {
    assert!(lines(
        "fn f() { let mut c = cmd.spawn().unwrap(); c.stdin.take(); let g = ChildGuard::new(c); }"
    )
    .is_empty());
    assert_eq!(
        lines("fn f() { let c = cmd.spawn().unwrap(); let d = ChildGuard::new(other); }"),
        vec![1]
    );
}

#[test]
fn a_spawn_handed_to_pump_passes() {
    assert!(lines("fn f() { pump(cmd.spawn().unwrap(), v, p); }").is_empty());
    assert!(lines(
        "fn f() { let c = cmd.spawn().unwrap(); moor_core::seam::proc::pump(c, v, p); }"
    )
    .is_empty());
}

#[test]
fn spawn_waited_in_the_same_expression_passes() {
    assert!(lines("fn f() { let o = cmd.spawn().unwrap().wait_with_output(); }").is_empty());
}

#[test]
fn helper_returning_a_bare_child_is_flagged() {
    assert_eq!(
        lines("fn spawn(&self) -> Child {\n self.cmd().spawn().unwrap()\n}"),
        vec![2]
    );
}

#[test]
fn a_spawn_with_arguments_is_not_a_process_spawn() {
    assert!(lines("fn f() { std::thread::Builder::new().spawn(|| ()).unwrap(); }").is_empty());
}

#[test]
fn production_code_is_not_checked() {
    let src = "fn f() { cmd.spawn().unwrap(); }\n#[cfg(test)]\nmod t { fn g() { cmd.spawn().unwrap(); } }";
    let ast = syn::parse_file(src).unwrap();
    assert_eq!(unguarded_spawns(&ast, false), vec![3]);
}

#[test]
fn a_test_fn_is_checked() {
    let src = "#[test]\nfn t() { cmd.spawn().unwrap(); }\nfn p() { cmd.spawn().unwrap(); }";
    let ast = syn::parse_file(src).unwrap();
    assert_eq!(unguarded_spawns(&ast, false), vec![2]);
}

fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in files {
        let p = dir.path().join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    dir
}

const BAD: &str = "fn f() { cmd.spawn().unwrap(); }";

#[test]
fn files_in_test_places_are_checked() {
    for path in [
        "crates/a/tests/x.rs",
        "crates/a/src/tests.rs",
        "crates/a/src/b/tests/c.rs",
    ] {
        let dir = tree(&[(path, BAD)]);
        let (_, bad) = check(dir.path(), "").unwrap();
        assert_eq!(bad.len(), 1, "{path}");
    }
}

#[test]
fn production_files_are_skipped() {
    let dir = tree(&[("crates/a/src/lib.rs", BAD)]);
    assert!(check(dir.path(), "").unwrap().1.is_empty());
}

#[test]
fn a_cfg_test_module_file_is_checked_with_its_children() {
    let dir = tree(&[
        ("crates/a/src/lib.rs", "#[cfg(test)]\nmod fixture;"),
        ("crates/a/src/fixture.rs", BAD),
        ("crates/a/src/fixture/inner.rs", BAD),
        ("crates/a/src/other.rs", BAD),
    ]);
    let (_, bad) = check(dir.path(), "").unwrap();
    assert_eq!(bad.len(), 2, "{bad:?}");
}

#[test]
fn an_allowlisted_file_is_exempt_and_a_stale_entry_is_an_error() {
    let dir = tree(&[
        ("crates/a/tests/x.rs", BAD),
        ("crates/a/tests/y.rs", "fn f() {}"),
    ]);
    assert!(check(dir.path(), "crates/a/tests/x.rs # reason")
        .unwrap()
        .1
        .is_empty());
    assert!(check(dir.path(), "crates/a/tests/y.rs").is_err());
    assert!(check(dir.path(), "crates/a/tests/none.rs").is_err());
}
