//! Readers that drain a child's pipe on their own thread.

use std::io::Read;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

use moor_core::seam::proc::OUTPUT_CAP;

use super::Capture;

/// Longest unfinished stderr line a watched call keeps.
const LINE_CAP: usize = 4096;

/// Detached reader: owns the pipe, fills the shared capture, signals EOF on `done`.
pub(super) fn drain<R: Read + Send + 'static>(
    pipe: Option<R>,
    done: Sender<bool>,
) -> Arc<Mutex<Capture>> {
    drain_lines(pipe, done, None)
}

/// [`drain`] that also sends each finished line, without its ending, on `lines`.
pub(super) fn drain_lines<R: Read + Send + 'static>(
    pipe: Option<R>,
    done: Sender<bool>,
    lines: Option<Sender<String>>,
) -> Arc<Mutex<Capture>> {
    let cap = Arc::new(Mutex::new(Capture::default()));
    let shared = Arc::clone(&cap);
    thread::spawn(move || {
        if let Some(mut r) = pipe {
            let mut chunk = [0_u8; 8192];
            let mut partial = Vec::new();
            while let Ok(n) = r.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let got = chunk.get(..n).unwrap_or_default();
                if let Ok(mut c) = shared.lock() {
                    let room = OUTPUT_CAP.saturating_sub(c.buf.len());
                    let take = n.min(room);
                    c.buf.extend_from_slice(got.get(..take).unwrap_or_default());
                    c.truncated |= take < n;
                }
                if let Some(tx) = &lines {
                    partial.extend_from_slice(got);
                    while let Some(end) = partial.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = partial.drain(..=end).collect();
                        let text = String::from_utf8_lossy(&line);
                        tx.send(text.trim_end().to_owned()).ok();
                    }
                    partial.truncate(LINE_CAP);
                }
            }
        }
        done.send(true).ok();
    });
    cap
}
