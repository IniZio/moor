use super::*;

#[test]
fn start_excludes_scratch_dirs_from_the_repo() {
    let w = world(FakeDriver::new());
    run(&w.cx, &req("demo")).unwrap();
    assert_exclude_call(&w, Path::new("/repo"));
}

#[test]
fn start_excludes_scratch_dirs_when_a_worktree_is_bound() {
    let w = world(FakeDriver::new());
    let made = w
        .surface
        .create_worktree(Path::new("/repo"), &box_name("pre"), None)
        .unwrap();
    let mut r = req("demo");
    r.worktree = Some(made.path.display().to_string());
    run(&w.cx, &r).unwrap();
    assert_exclude_call(&w, &made.path);
}

#[test]
fn exclude_failure_stops_start_before_any_worktree_exists() {
    let w = world(FakeDriver::new());
    break_exclude(&w);
    let err = run(&w.cx, &req("demo")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert!(w.surface.calls().is_empty(), "{:?}", w.surface.calls());
    let name = box_name("demo");
    assert!(w
        .cx
        .store()
        .unwrap()
        .open_task_by_name(&name)
        .unwrap()
        .is_none());
}

#[test]
fn attach_excludes_scratch_dirs_again() {
    let w = world(FakeDriver::new());
    let first = run(&w.cx, &req("demo")).unwrap();
    let wt = std::path::PathBuf::from(first.box_row.worktree.unwrap());
    let before = exclude_calls(&w);
    assert!(run(&w.cx, &req("demo")).unwrap().attached);
    assert_eq!(exclude_calls(&w), before + 1);
    assert_exclude_call(&w, &wt);
}

#[test]
fn attach_warns_when_the_exclude_fails_and_still_attaches() {
    let w = world(FakeDriver::new());
    run(&w.cx, &req("demo")).unwrap();
    break_exclude(&w);
    let again = run(&w.cx, &req("demo")).unwrap();
    assert!(again.attached);
    let warnings = w.cx.take_warnings();
    assert!(
        warnings.iter().any(|x| x.code == "exclude_failed"),
        "{warnings:?}"
    );
}

/// Makes the exclude file unreadable by putting a directory where it belongs.
fn break_exclude(w: &World) {
    let path = w.dir.path().join("exclude");
    let _ = std::fs::remove_file(&path);
    std::fs::create_dir(&path).unwrap();
}

/// How many times git was asked for `info/exclude`.
fn exclude_calls(w: &World) -> usize {
    w.proc
        .calls()
        .into_iter()
        .filter(|c| c.args == ["rev-parse", "--git-path", "info/exclude"])
        .count()
}

/// Asserts git was asked for `info/exclude` from inside `dir`.
fn assert_exclude_call(w: &World, dir: &Path) {
    let hit = w.proc.calls().into_iter().any(|c| {
        c.args == ["rev-parse", "--git-path", "info/exclude"] && c.cwd.as_deref() == Some(dir)
    });
    assert!(hit, "no exclude lookup in {}", dir.display());
}
