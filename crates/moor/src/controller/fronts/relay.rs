//! Copies a front's output into the controller log, a line at a time.

use std::io::{BufRead, BufReader, Read};
use std::thread;

use crate::controller::{log, Sink};

/// Longest part of a line that is logged; the rest of that line is read and dropped.
const MAX_LINE: u64 = 64 * 1024;

/// Logs each line of `stream`, a front's output, to `sink` until the front closes it. Bytes
/// that are not UTF-8 are replaced, and a line over [`MAX_LINE`] is cut and says so, so a
/// front can neither end the log nor grow the controller by what it prints.
pub(super) fn relay_lines(stream: impl Read + Send + 'static, ns: &str, sink: &Sink) {
    let (sink, ns) = (sink.clone(), ns.to_owned());
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = (&mut reader).take(MAX_LINE).read_until(b'\n', &mut line);
            if !matches!(read, Ok(n) if n > 0) {
                return;
            }
            let ended = line.last() == Some(&b'\n');
            if ended {
                line.pop();
            }
            let mut text = String::from_utf8_lossy(&line).into_owned();
            if !ended && line.len() as u64 >= MAX_LINE {
                text.push_str(" [line cut at 64 KiB]");
                skip_line(&mut reader);
            }
            log(&sink, &format!("front {ns}: {text}"));
        }
    });
}

/// Reads and drops bytes up to and including the next newline.
fn skip_line(reader: &mut impl BufRead) {
    loop {
        let Ok(buf) = reader.fill_buf() else { return };
        if buf.is_empty() {
            return;
        }
        let (used, found) = buf
            .iter()
            .position(|b| *b == b'\n')
            .map_or((buf.len(), false), |i| (i + 1, true));
        reader.consume(used);
        if found {
            return;
        }
    }
}
