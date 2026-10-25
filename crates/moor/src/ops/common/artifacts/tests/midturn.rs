//! Tests for collecting while the agent's turn is still running.

use std::time::Duration;

use super::{fx, line, row, versions, Fx};
use crate::ops::common::artifacts::collect_working;

fn pass(f: &Fx) {
    collect_working(&f.cx, f.cx.store().unwrap(), &|| false);
}

fn later(f: &Fx) {
    f.cx.clock().sleep(Duration::from_secs(31));
}

fn calls(f: &Fx) -> usize {
    f.driver.box_calls().len()
}

#[test]
fn a_file_is_stored_once_two_passes_saw_it_unchanged() {
    let f = fx();
    f.driver.set_copy_text("hello");
    f.driver.push_artifact_index(line("a.md", b"hello"));
    pass(&f);
    assert!(versions(&f).is_empty());
    later(&f);
    f.driver.push_artifact_index(line("a.md", b"hello"));
    pass(&f);
    assert_eq!(versions(&f), vec![("a.md".to_owned(), 1)]);
}

#[test]
fn a_file_still_growing_is_not_stored() {
    let f = fx();
    f.driver.set_copy_text("hello");
    f.driver.push_artifact_index(line("a.md", b"he"));
    pass(&f);
    later(&f);
    f.driver.push_artifact_index(line("a.md", b"hello"));
    pass(&f);
    assert!(versions(&f).is_empty());
    later(&f);
    f.driver.push_artifact_index(line("a.md", b"hello"));
    pass(&f);
    assert_eq!(versions(&f), vec![("a.md".to_owned(), 1)]);
}

#[test]
fn a_box_is_collected_at_most_once_per_interval() {
    let f = fx();
    pass(&f);
    let after_first = calls(&f);
    pass(&f);
    assert_eq!(calls(&f), after_first);
    later(&f);
    pass(&f);
    assert!(calls(&f) > after_first);
}

#[test]
fn an_unchanged_file_does_not_publish_another_version() {
    let f = fx();
    f.driver.set_copy_text("hello");
    for _ in 0..4 {
        f.driver.push_artifact_index(line("a.md", b"hello"));
        pass(&f);
        later(&f);
    }
    assert_eq!(versions(&f).len(), 1);
}

#[test]
fn a_quiet_box_is_not_collected_mid_turn() {
    let f = fx();
    f.cx.store()
        .unwrap()
        .writer()
        .execute("UPDATE sessions SET state = 'idle'", [])
        .unwrap();
    assert!(row(&f).session.is_quiet());
    let before = calls(&f);
    pass(&f);
    assert_eq!(calls(&f), before);
}

#[test]
fn a_new_turn_starts_without_the_old_turns_index() {
    let f = fx();
    f.driver.set_copy_text("hello");
    f.driver.push_artifact_index(line("a.md", b"hello"));
    pass(&f);
    f.cx.store()
        .unwrap()
        .writer()
        .execute("UPDATE sessions SET state = 'idle'", [])
        .unwrap();
    later(&f);
    pass(&f);
    f.cx.store()
        .unwrap()
        .writer()
        .execute("UPDATE sessions SET state = 'working'", [])
        .unwrap();
    later(&f);
    f.driver.push_artifact_index(line("a.md", b"hello"));
    pass(&f);
    assert!(versions(&f).is_empty());
}
