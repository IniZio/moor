//! Bounded decoder for a chunked request body.

use std::io::{self, BufRead, Read};

/// Longest chunk-size line, extensions included.
const MAX_LINE: u64 = 1024;
/// Most trailer lines accepted after the last chunk.
const MAX_TRAILERS: usize = 32;

/// Where the decoder is in the chunk framing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// Next bytes are a chunk-size line.
    Size,
    /// Inside chunk data.
    Data,
    /// The last chunk and trailers were consumed.
    Done,
}

/// Reads the payload of a chunked body, dropping the framing and trailers.
pub(super) struct ChunkedReader<R: BufRead> {
    /// Framed source.
    inner: R,
    /// Position in the framing.
    state: State,
    /// Data bytes left in the current chunk.
    left: u64,
}

impl<R: BufRead> ChunkedReader<R> {
    /// Wraps a framed source.
    pub(super) const fn new(inner: R) -> Self {
        Self {
            inner,
            state: State::Size,
            left: 0,
        }
    }

    /// Reads one CRLF- or LF-terminated line of at most `MAX_LINE` bytes,
    /// returning it without the terminator.
    fn line(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        let n = (&mut self.inner)
            .take(MAX_LINE)
            .read_until(b'\n', &mut buf)?;
        if n == 0 || buf.pop() != Some(b'\n') {
            return Err(bad("chunk line too long or truncated"));
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        Ok(buf)
    }

    /// Parses a chunk-size line, ignoring extensions.
    fn chunk_size(&mut self) -> io::Result<u64> {
        let line = self.line()?;
        let digits = line.split(|b| *b == b';').next().unwrap_or_default();
        let digits = digits.trim_ascii();
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_hexdigit) {
            return Err(bad("invalid chunk size"));
        }
        let text = std::str::from_utf8(digits).map_err(|_| bad("invalid chunk size"))?;
        u64::from_str_radix(text, 16).map_err(|_| bad("invalid chunk size"))
    }

    /// Consumes the trailer lines up to the blank line.
    fn trailers(&mut self) -> io::Result<()> {
        for _ in 0..=MAX_TRAILERS {
            if self.line()?.is_empty() {
                return Ok(());
            }
        }
        Err(bad("too many trailers"))
    }
}

/// Builds the error for malformed framing.
fn bad(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

impl<R: BufRead> Read for ChunkedReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            match self.state {
                State::Done => return Ok(0),
                State::Size => {
                    self.left = self.chunk_size()?;
                    if self.left == 0 {
                        self.trailers()?;
                        self.state = State::Done;
                    } else {
                        self.state = State::Data;
                    }
                }
                State::Data => {
                    let want = usize::try_from(self.left).map_or(out.len(), |l| l.min(out.len()));
                    let dst = out.get_mut(..want).ok_or_else(|| bad("buffer"))?;
                    let n = self.inner.read(dst)?;
                    if n == 0 {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                    self.left -= n as u64;
                    if self.left == 0 {
                        if !self.line()?.is_empty() {
                            return Err(bad("missing chunk terminator"));
                        }
                        self.state = State::Size;
                    }
                    return Ok(n);
                }
            }
        }
    }
}
