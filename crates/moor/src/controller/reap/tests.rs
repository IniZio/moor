//! Tests for the controller's lease reaper loop.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::cell::RefCell;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{run_loop, EVERY};
use crate::controller::Sink;

/// A sink that appends into a shared buffer.
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn sink() -> (Sink, Arc<Mutex<Vec<u8>>>) {
    let buf = Arc::new(Mutex::new(Vec::new()));
    (Arc::new(Mutex::new(Buf(buf.clone()))), buf)
}

#[test]
fn first_tick_runs_before_any_sleep() {
    let (sink, _) = sink();
    let sleeps = RefCell::new(Vec::new());
    let slept_at_first_tick = RefCell::new(None);
    run_loop(
        |d| sleeps.borrow_mut().push(d),
        EVERY,
        &sink,
        Some(1),
        || {
            *slept_at_first_tick.borrow_mut() = Some(sleeps.borrow().len());
            Vec::new()
        },
    );
    assert_eq!(slept_at_first_tick.into_inner(), Some(0));
    assert!(sleeps.borrow().is_empty());
}

#[test]
fn n_rounds_sleep_n_minus_one_gaps() {
    let (sink, _) = sink();
    let mut sleeps = Vec::new();
    let mut calls = 0;
    run_loop(
        |d| sleeps.push(d),
        EVERY,
        &sink,
        Some(4),
        || {
            calls += 1;
            Vec::new()
        },
    );
    assert_eq!(calls, 4);
    assert_eq!(sleeps, vec![EVERY; 3]);
    assert_eq!(EVERY, Duration::from_secs(600));
}

#[test]
fn tick_lines_are_logged_to_the_sink() {
    let (sink, buf) = sink();
    run_loop(|_| {}, EVERY, &sink, Some(2), || vec!["released x".into()]);
    let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert_eq!(
        out,
        "moor controller: reap: released x\nmoor controller: reap: released x\n"
    );
}
