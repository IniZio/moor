//! Server-sent events from a blocking reader, as the JSON messages their `data` carries.
//!
//! `sse-stream` reads an async byte stream; feeding it a blocking `ureq` body would take a
//! runtime and a bridge thread for a parser of a few lines, so this reads lines itself.

use std::io::{self, BufRead};

use serde_json::Value;

/// The JSON messages of an event stream, one per event.
pub struct Messages<R> {
    /// Where the stream is read from.
    reader: R,
}

impl<R: BufRead> Messages<R> {
    /// The messages `reader` carries.
    #[must_use]
    pub const fn new(reader: R) -> Self {
        Self { reader }
    }
}

impl<R: BufRead> Iterator for Messages<R> {
    type Item = io::Result<Value>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut data = String::new();
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Err(e) => return Some(Err(e)),
                Ok(0) => return None,
                Ok(_) => {}
            }
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if let Ok(message) = serde_json::from_str(&data) {
                    return Some(Ok(message));
                }
                data.clear();
            } else if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Messages;

    #[test]
    fn comments_and_other_fields_are_skipped_and_data_lines_join() {
        let raw = ": keepalive\n\nevent: message\nid: 3\ndata: {\"a\":\ndata: 1}\n\ndata: {\"b\":2}\r\n\r\n";
        let got: Vec<_> = Messages::new(raw.as_bytes()).flatten().collect();
        assert_eq!(
            got,
            [serde_json::json!({"a": 1}), serde_json::json!({"b": 2})]
        );
    }
}
