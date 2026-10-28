//! The thread that owns one pane's terminal emulator.
//!
//! libghostty-vt handles cannot leave the thread that made them, so every read of the
//! screen is a message to that thread.

use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::thread;

use ghostty_vt::fmt::{Format, Formatter, FormatterOptions};
use ghostty_vt::{Event, Terminal, TerminalOptions};
use portable_pty::PtySize;

/// A request to a pane's screen thread.
pub(super) enum Msg {
    /// Bytes the pane printed.
    Output(Vec<u8>),
    /// The last this many lines of the screen, sent back on the channel.
    Tail(usize, mpsc::Sender<String>),
}

/// The last `n` lines of `text`, each without trailing blanks, and no blank lines at the end.
pub(super) fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    lines
        .get(end.saturating_sub(n)..end)
        .unwrap_or_default()
        .join("\n")
}

/// The active screen of `term` as plain text.
fn screen_text(term: &Terminal) -> String {
    let text = Formatter::new(term, FormatterOptions::new().with_format(Format::Plain))
        .and_then(|mut f| f.format_alloc());
    text.map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// Starts the threads that read `output` into a `size` terminal and answer [`Msg::Tail`].
///
/// Replies the terminal owes the program (cursor reports and the like) go to `keys`.
pub(super) fn start(
    mut output: Box<dyn Read + Send>,
    keys: Arc<Mutex<Box<dyn Write + Send>>>,
    size: PtySize,
) -> mpsc::Sender<Msg> {
    let (tx, rx) = mpsc::channel();
    let feed = tx.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n @ 1..) = output.read(&mut buf) {
            let chunk = buf.get(..n).unwrap_or_default().to_vec();
            if feed.send(Msg::Output(chunk)).is_err() {
                break;
            }
        }
    });
    thread::spawn(move || {
        let opts = TerminalOptions {
            cols: size.cols,
            rows: size.rows,
            max_scrollback: 1000,
        };
        let Ok(mut term) = Terminal::new(opts) else {
            return;
        };
        for msg in rx {
            match msg {
                Msg::Output(bytes) => {
                    term.vt_write(&bytes);
                    let replies: Vec<Vec<u8>> = term
                        .events()
                        .filter_map(|e| match e {
                            Event::PtyWrite(b) => Some(b),
                            _ => None,
                        })
                        .collect();
                    if !replies.is_empty() {
                        let mut keys = keys.lock().unwrap_or_else(PoisonError::into_inner);
                        for b in replies {
                            let _ = keys.write_all(&b);
                        }
                        let _ = keys.flush();
                    }
                }
                Msg::Tail(n, reply) => {
                    let _ = reply.send(last_lines(&screen_text(&term), n));
                }
            }
        }
    });
    tx
}
