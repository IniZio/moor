//! The line the exec prelude prints when a Box's egress forwarder has no config. moor strips it
//! from every exec's stderr before anyone sees it and remembers the Box, so the Box's next exec
//! gets the forwarder delivered again.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::sync::{LazyLock, Mutex, PoisonError};

use moor_core::exec_env::FORWARDER_LOST;
use moor_core::name::BoxHandle;
use moor_core::seam::proc::Hidden;

/// Boxes whose prelude printed the line since their last delivery. Drivers are made per
/// request, so the set is the process's.
static LOST: LazyLock<Mutex<BTreeSet<String>>> = LazyLock::new(Mutex::default);

/// Remembers that the Box behind `handle` has lost its forwarder config.
pub(super) fn note(handle: &BoxHandle) {
    LOST.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(handle.as_str().to_owned());
}

/// Whether the Box behind `handle` lost its forwarder config; forgets it once asked.
pub(super) fn take(handle: &BoxHandle) -> bool {
    LOST.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(handle.as_str())
}

/// The line, for a stderr gate to hide.
pub(super) fn hidden() -> Hidden {
    Hidden::new(FORWARDER_LOST)
}

/// Notes the Box when captured `stderr` holds the line.
pub(super) fn scan(handle: &BoxHandle, stderr: &str) {
    if stderr
        .lines()
        .any(|l| l.trim_end_matches('\r') == FORWARDER_LOST)
    {
        note(handle);
    }
}

/// Passes a stream through to `inner` without the line. Bytes that could still start the line
/// are held only until they can no longer, so a prompt with no newline is not delayed.
pub(super) struct Strip<'w, W: Write + ?Sized> {
    /// Where the rest goes.
    inner: &'w mut W,
    /// Bytes from the start of the current line that are still a prefix of the line.
    held: Vec<u8>,
    /// Whether the next byte starts a line.
    at_start: bool,
    /// Whether the line went by.
    seen: bool,
}

impl<'w, W: Write + ?Sized> Strip<'w, W> {
    /// A filter in front of `inner`.
    pub(super) const fn new(inner: &'w mut W) -> Self {
        Self {
            inner,
            held: Vec::new(),
            at_start: true,
            seen: false,
        }
    }

    /// Writes what is still held and notes the Box behind `handle` when the line went by.
    pub(super) fn finish(mut self, handle: &BoxHandle) {
        let held = std::mem::take(&mut self.held);
        drop(
            self.inner
                .write_all(&held)
                .and_then(|()| self.inner.flush()),
        );
        if self.seen {
            note(handle);
        }
    }
}

impl<W: Write + ?Sized> Write for Strip<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let line = FORWARDER_LOST.as_bytes();
        let mut out = Vec::with_capacity(buf.len());
        for &b in buf {
            if self.at_start || !self.held.is_empty() {
                let crlf = b == b'\r' && self.held == line;
                if b == b'\n' && self.held.strip_suffix(b"\r").unwrap_or(&self.held) == line {
                    self.held.clear();
                    self.seen = true;
                    self.at_start = true;
                    continue;
                }
                self.held.push(b);
                if crlf || line.starts_with(&self.held) {
                    self.at_start = false;
                    continue;
                }
                out.append(&mut self.held);
            } else {
                out.push(b);
            }
            self.at_start = b == b'\n';
        }
        self.inner.write_all(&out)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests;
