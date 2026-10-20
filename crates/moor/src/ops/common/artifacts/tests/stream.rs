//! Tests for collecting artifacts through one batched index and one file stream.

use moor_core::seam::driver::DriverError;

use super::{codes, collect_calls, fx_with, line, run, sha, versions, warnings, Fx};
use crate::testing::driver::FakeDriver;

const BIN: [u8; 5] = [0xff, 0xfe, 0x00, 0x80, 0x0a];

fn three() -> Fx {
    let f = fx_with(
        FakeDriver::new()
            .box_file("a.md", b"alpha")
            .box_file("b.bin", &BIN)
            .box_file("c.md", b"gamma"),
    );
    push_three(&f);
    f
}

fn push_three(f: &Fx) {
    let mut out = line("a.md", b"alpha");
    out.push_str(&line("b.bin", &BIN));
    out.push_str(&line("c.md", b"gamma"));
    f.driver.push_artifact_index(out);
}

#[test]
fn many_files_take_one_batch_and_one_stream() {
    let f = three();
    let before = f.driver.box_calls().len();
    assert_eq!(run(&f), 3);
    assert!(codes(&f).is_empty());
    assert!(f.driver.copies().is_empty());
    assert_eq!(collect_calls(&f, before), vec!["batch", "stream_files"]);
    let store = f.cx.store().unwrap();
    let rows = store.artifacts(f.id).unwrap();
    assert_eq!(rows.len(), 3);
    assert_stored(&rows, "a.md", b"alpha");
    assert_stored(&rows, "b.bin", &BIN);
    assert_stored(&rows, "c.md", b"gamma");
    assert_one_stream(&f);
}

fn assert_stored(rows: &[crate::store::artifacts::ArtifactRow], name: &str, bytes: &[u8]) {
    let r = rows.iter().find(|r| r.name == name).unwrap();
    assert_eq!(r.size, bytes.len() as u64);
    assert_eq!(r.sha256, sha(bytes));
    assert_eq!(std::fs::read(&r.path).unwrap(), bytes);
}

fn assert_one_stream(f: &Fx) {
    let streams = f.driver.streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].1.as_str(), "/work/box-1/.moor/artifacts");
    let mut names = streams[0].2.clone();
    names.sort();
    assert_eq!(names, vec!["a.md", "b.bin", "c.md"]);
}

#[test]
fn a_broken_stream_keeps_what_arrived_and_the_rest_is_collected_next_time() {
    let f = three();
    f.driver.fail_stream_after(
        1,
        DriverError::Failed {
            summary: "pipe closed".into(),
        },
    );
    assert_eq!(run(&f), 1);
    assert_two_skips(&f);
    assert_eq!(versions(&f).len(), 1);
    push_three(&f);
    assert_eq!(run(&f), 2);
    let mut got = versions(&f);
    got.sort();
    assert_eq!(got.len(), 3);
    assert!(got.iter().all(|(_, v)| *v == 1));
}

fn assert_two_skips(f: &Fx) {
    let w = warnings(f);
    assert_eq!(w.len(), 2);
    for x in &w {
        assert_eq!(x.code, "artifact_skipped");
        assert!(x.message.contains("could not be copied"));
        assert!(x.message.contains("tried next time"));
    }
}

#[test]
fn a_box_without_a_stream_falls_back_to_one_copy_per_file() {
    let f = fx_with(FakeDriver::new());
    f.driver.set_copy_text("hello");
    let mut out = line("a.md", b"hello");
    out.push_str(&line("b.md", b"hello"));
    f.driver.push_artifact_index(out);
    assert_eq!(run(&f), 2);
    assert!(codes(&f).is_empty());
    assert_eq!(f.driver.copies().len(), 2);
    assert_eq!(versions(&f).len(), 2);
}

#[test]
fn streamed_bytes_that_differ_from_the_index_are_refused() {
    let f = fx_with(FakeDriver::new().box_file("a.md", b"other"));
    f.driver.push_artifact_index(line("a.md", b"hello"));
    assert_eq!(run(&f), 0);
    let w = warnings(&f);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].code, "artifact_skipped");
    assert!(w[0].message.contains("changed while it was copied"));
    assert!(versions(&f).is_empty());
}

#[test]
fn a_streamed_non_regular_entry_is_skipped() {
    let f = fx_with(FakeDriver::new().box_special("a.md"));
    f.driver.push_artifact_index(line("a.md", b"hello"));
    assert_eq!(run(&f), 0);
    let w = warnings(&f);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].code, "artifact_skipped");
    assert!(w[0].message.contains("was not a plain file"));
    assert!(versions(&f).is_empty());
}

#[test]
fn an_indexed_file_the_stream_never_hands_over_is_reported() {
    let f = fx_with(
        FakeDriver::new()
            .box_file("a.md", b"alpha")
            .box_file("b.bin", &BIN),
    );
    push_three(&f);
    assert_eq!(run(&f), 2);
    let w = warnings(&f);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].code, "artifact_skipped");
    assert!(w[0].message.contains("c.md") && w[0].message.contains("was not received"));
    assert_eq!(versions(&f).len(), 2);
}
