use super::*;

#[test]
fn the_warning_names_the_box_log_and_ends_with_its_tail() {
    let d = tempfile::tempdir().unwrap();
    let body: Vec<String> = (1..=30).map(|i| format!("out {i}")).collect();
    let log = format!("moor setup: $ bad\n{}\n", body.join("\n"));
    let driver = FakeDriver::new().exec_results(exits(&[2])).copy_text(&log);
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["bad"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    let path = format!("{}/.moor/setup.log", r.workdir.as_str());
    assert!(w.message.contains(&path), "{}", w.message);
    assert!(w.message.contains("out 30"), "{}", w.message);
    assert!(w.message.contains("out 11"), "{}", w.message);
    assert!(!w.message.contains("out 10\n"), "{}", w.message);
}

#[test]
fn a_log_with_invalid_utf8_still_finds_the_failing_line_and_tail() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .copy_text(b"moor setup: $ a\nmoor setup: $ b\nbad \xff byte\n");
    let r = rig(d.path(), driver);
    let got = run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a", "b", "c"], &[]),
        SyncMode::Full,
    );
    assert_eq!(got.view().line.as_deref(), Some("b"));
    let w = only(r.cx.take_warnings());
    assert!(w.message.contains("bad"), "{}", w.message);
}

#[test]
fn a_redrawn_line_shows_only_its_final_frame() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .copy_text("moor setup: $ a\n10%\r50%\r100%\n");
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    assert!(w.message.ends_with("\n100%"), "{}", w.message);
    assert!(!w.message.contains("50%"), "{}", w.message);
    assert!(!w.message.contains('\r'), "{}", w.message);
}

#[test]
fn a_progress_line_does_not_push_earlier_context_out_of_the_tail() {
    let d = tempfile::tempdir().unwrap();
    let body: Vec<String> = (1..=19).map(|i| format!("out {i}")).collect();
    let log = format!("moor setup: $ a\n{}\n1%\r2%\r3%\r4%\r5%\n", body.join("\n"));
    let driver = FakeDriver::new().exec_results(exits(&[1])).copy_text(log);
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    assert!(w.message.contains("last 20 lines"), "{}", w.message);
    assert!(w.message.contains("out 1\n"), "{}", w.message);
    assert!(w.message.ends_with("\n5%"), "{}", w.message);
}

#[test]
fn long_tail_lines_are_capped() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .copy_text(format!("moor setup: $ a\n{}\n", "x".repeat(1000)));
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    let last = w.message.lines().last().unwrap();
    assert_eq!(last.chars().count(), 300);
    assert!(last.ends_with('…'), "{last}");
}

#[test]
fn control_characters_are_stripped_but_tabs_stay() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .copy_text("moor setup: $ a\n\u{1b}[31mred\u{1b}[0m\u{1b}]0;title\u{7}\tok\u{7}\n");
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    assert!(w.message.ends_with("\nred\tok"), "{}", w.message);
    assert!(!w.message.contains('\u{1b}'), "{}", w.message);
}

#[test]
fn an_unreadable_log_still_gets_its_path_in_the_warning() {
    let d = tempfile::tempdir().unwrap();
    let driver = FakeDriver::new()
        .exec_results(exits(&[1]))
        .fail_copy_out(DriverError::NotFound);
    let r = rig(d.path(), driver);
    run_setup(
        &r.cx,
        &r.handle,
        &r.workdir,
        d.path(),
        &cfg(&["a"], &[]),
        SyncMode::Full,
    );
    let w = only(r.cx.take_warnings());
    let path = format!("{}/.moor/setup.log", r.workdir.as_str());
    assert!(w.message.contains(&path), "{}", w.message);
}
