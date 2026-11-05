//! Tests for stripping the lost-forwarder line.

use std::io::Write;

use moor_core::exec_env::FORWARDER_LOST;
use moor_core::name::BoxHandle;

use super::{scan, take, Strip};

/// What a user reads after `chunks` go through the filter, and whether the Box was noted.
fn through(handle: &str, chunks: &[&[u8]]) -> (String, bool) {
    let handle = BoxHandle::new(handle);
    let mut seen: Vec<u8> = Vec::new();
    let mut strip = Strip::new(&mut seen);
    for chunk in chunks {
        strip.write_all(chunk).unwrap();
    }
    strip.finish(&handle);
    (String::from_utf8(seen).unwrap(), take(&handle))
}

#[test]
fn the_line_never_reaches_the_user_and_the_box_is_noted_once() {
    let text = format!("before\n{FORWARDER_LOST}\nafter\n");
    let (shown, noted) = through("p/strip-a", &[text.as_bytes()]);
    assert_eq!(shown, "before\nafter\n");
    assert!(noted);
    assert!(
        !take(&BoxHandle::new("p/strip-a")),
        "asking forgets the report"
    );
}

#[test]
fn the_line_is_caught_when_split_across_writes_and_with_crlf() {
    let line = format!("{FORWARDER_LOST}\r\n");
    let (a, b) = line.as_bytes().split_at(7);
    let (shown, noted) = through("p/strip-b", &[b"x\n", a, b, b"y"]);
    assert_eq!(shown, "x\ny");
    assert!(noted);
}

#[test]
fn other_output_passes_whole_and_unheld() {
    let (shown, noted) = through(
        "p/strip-c",
        &[
            b"password: ",
            b"moor-egress-x\n",
            format!("say {FORWARDER_LOST}\n").as_bytes(),
        ],
    );
    assert_eq!(
        shown,
        format!("password: moor-egress-x\nsay {FORWARDER_LOST}\n")
    );
    assert!(!noted);
}

#[test]
fn a_prompt_with_no_newline_is_written_at_once() {
    let handle = BoxHandle::new("p/strip-d");
    let mut seen: Vec<u8> = Vec::new();
    let mut strip = Strip::new(&mut seen);
    strip.write_all(b"Continue? ").unwrap();
    drop(strip);
    assert_eq!(seen, b"Continue? ");
    assert!(!take(&handle));
}

#[test]
fn captured_stderr_is_scanned() {
    let handle = BoxHandle::new("p/strip-e");
    scan(&handle, &format!("x\n{FORWARDER_LOST}\r\n"));
    assert!(take(&handle));
    scan(&handle, "nothing here\n");
    assert!(!take(&handle));
}
