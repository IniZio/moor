//! Cap enforcement for [`CappedReader`].

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{is_cap_exceeded, CappedReader};

/// Cap used by every test.
const CAP: u64 = 10;

/// Inner reader that hands out one byte per call.
struct OneByte<'a>(&'a [u8]);

impl Read for OneByte<'_> {
    /// Copies the next byte, or reports end of input.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some((first, rest)) = self.0.split_first() else {
            return Ok(0);
        };
        let Some(slot) = buf.first_mut() else {
            return Ok(0);
        };
        *slot = *first;
        self.0 = rest;
        Ok(1)
    }
}

/// Builds a reader over `inner` with the shared flag.
fn capped<R: Read>(inner: R) -> (CappedReader<R>, Arc<AtomicBool>) {
    let flag = Arc::new(AtomicBool::new(false));
    (CappedReader::new(inner, CAP, Arc::clone(&flag)), flag)
}

/// A body under the cap passes through whole.
#[test]
fn under_cap_passes() {
    let (mut r, flag) = capped(&b"abcde"[..]);
    let mut out = Vec::new();
    r.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"abcde");
    assert!(!flag.load(Ordering::SeqCst));
}

/// Exactly the cap followed by end of input is accepted.
#[test]
fn exactly_cap_then_eof_ok() {
    let (mut r, flag) = capped(&b"0123456789"[..]);
    let mut out = Vec::new();
    r.read_to_end(&mut out).unwrap();
    assert_eq!(out.len(), 10);
    assert!(!flag.load(Ordering::SeqCst));
}

/// One byte over errors, trips the flag and returns at most the cap.
#[test]
fn one_over_errors_and_trips() {
    let (mut r, flag) = capped(&b"0123456789X"[..]);
    let mut out = Vec::new();
    let err = r.read_to_end(&mut out).unwrap_err();
    assert!(is_cap_exceeded(&err));
    assert!(flag.load(Ordering::SeqCst));
    assert!(out.len() <= 10);
}

/// A large single read never yields more than the cap in total.
#[test]
fn big_buffer_gets_no_extra_bytes() {
    let data = [7_u8; 64];
    let (mut r, flag) = capped(&data[..]);
    let mut buf = [0_u8; 64];
    let mut total = 0;
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) => {
                assert!(is_cap_exceeded(&e));
                break;
            }
        }
    }
    assert!(total <= 10);
    assert!(flag.load(Ordering::SeqCst));
}

/// Only the cap error is recognised, not other io errors.
#[test]
fn detects_only_cap_error() {
    let other = io::Error::new(io::ErrorKind::InvalidData, "different");
    assert!(!is_cap_exceeded(&other));
    assert!(!is_cap_exceeded(&io::Error::from(
        io::ErrorKind::UnexpectedEof
    )));
}

/// After the cap trips, every later read fails the same way.
#[test]
fn error_is_sticky() {
    let (mut r, _flag) = capped(&b"0123456789XYZ"[..]);
    let mut buf = [0_u8; 32];
    let mut first = None;
    for _ in 0..4 {
        if let Err(e) = r.read(&mut buf) {
            first = Some(e);
            break;
        }
    }
    assert!(is_cap_exceeded(&first.unwrap()));
    for _ in 0..3 {
        let err = r.read(&mut buf).unwrap_err();
        assert!(is_cap_exceeded(&err));
    }
}

/// One-byte reads at the cap are accepted.
#[test]
fn one_byte_reads_at_cap_ok() {
    let (mut r, flag) = capped(OneByte(b"0123456789"));
    let mut out = Vec::new();
    r.read_to_end(&mut out).unwrap();
    assert_eq!(out.len(), 10);
    assert!(!flag.load(Ordering::SeqCst));
}

/// One-byte reads past the cap error with exactly the cap returned.
#[test]
fn one_byte_reads_over_cap_error() {
    let (mut r, flag) = capped(OneByte(b"0123456789X"));
    let mut out = Vec::new();
    let err = r.read_to_end(&mut out).unwrap_err();
    assert!(is_cap_exceeded(&err));
    assert_eq!(out.len(), 10);
    assert!(flag.load(Ordering::SeqCst));
}
