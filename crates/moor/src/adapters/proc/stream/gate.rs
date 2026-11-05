//! The stderr gate: forwards only the lines between two marker lines, never a hidden one.

use moor_core::seam::proc::StderrGate;

/// Longest line the gate buffers; a longer one is cut here and handled as two.
const LINE_CAP: usize = 8 * 1024;

/// Where a gated stream stands relative to its marker lines.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The begin marker has not appeared.
    Before,
    /// Between the markers: lines are forwarded.
    Open,
    /// The end marker has passed.
    After,
}

/// Line filter that forwards only what lies between two marker lines.
///
/// The writer puts a newline before the end marker so the marker gets a line of its own after
/// output with no trailing newline. An empty line is therefore held until the next line: it is
/// dropped when the end marker follows it.
pub(super) struct Gate {
    /// The marker lines, matched whole.
    markers: StderrGate,
    /// Position relative to the markers.
    phase: Phase,
    /// Bytes of the line being read.
    pub(super) line: Vec<u8>,
    /// An empty line held back, as read.
    held: Vec<u8>,
}

impl Gate {
    /// A closed gate over `markers`.
    pub(super) const fn new(markers: StderrGate) -> Self {
        Self {
            markers,
            phase: Phase::Before,
            line: Vec::new(),
            held: Vec::new(),
        }
    }

    /// Takes the next bytes and returns the ones to forward.
    pub(super) fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for &b in chunk {
            self.line.push(b);
            if b == b'\n' || self.line.len() >= LINE_CAP {
                self.finish_line(&mut out);
            }
        }
        out
    }

    /// Handles a final line that never got its newline; call at EOF.
    pub(super) fn finish(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if !self.line.is_empty() {
            self.finish_line(&mut out);
        }
        out.append(&mut self.held);
        out
    }

    /// Classifies the buffered line, appending it to `out` when it is forwarded.
    fn finish_line(&mut self, out: &mut Vec<u8>) {
        let line = std::mem::take(&mut self.line);
        let text = line.strip_suffix(b"\n").unwrap_or(&line);
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        match self.phase {
            Phase::Before if text == self.markers.begin.as_bytes() => self.phase = Phase::Open,
            Phase::Open if text == self.markers.end.as_bytes() => {
                self.held.clear();
                self.phase = Phase::After;
            }
            Phase::Open
                if self
                    .markers
                    .hide
                    .as_ref()
                    .is_some_and(|h| text == h.line.as_bytes()) =>
            {
                if let Some(h) = &self.markers.hide {
                    h.note();
                }
            }
            Phase::Open => {
                out.append(&mut self.held);
                if text.is_empty() && line.ends_with(b"\n") {
                    self.held = line;
                } else {
                    out.extend_from_slice(&line);
                }
            }
            Phase::Before | Phase::After => {}
        }
    }
}
