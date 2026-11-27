//! A request body read as payload, chunked framing removed, and the rest of a push sent on.

use std::io::{self, BufRead, ErrorKind, Read, Write};

use super::http::Framing;

/// Longest chunk-size or trailer line read, line ending included.
const MAX_LINE: u64 = 1024;
/// Most trailer lines read after the last chunk.
const MAX_TRAILERS: usize = 64;
/// Size of one copy of the rest of a push.
const CHUNK: usize = 1 << 14;

/// Where a body stands while it is read: what is left of the current chunk or length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BodyState {
    /// Payload bytes left before the next chunk-size line, or before the end.
    left: u64,
    /// Whether the body is chunked.
    chunked: bool,
    /// Whether a chunk has been read, so a chunk-size line follows a line ending.
    begun: bool,
    /// Whether the last chunk and its trailers have been read.
    done: bool,
}

/// The payload of a body read from `r`.
pub(crate) struct Body<'a, R> {
    /// The client.
    r: &'a mut R,
    /// Where the body stands.
    s: BodyState,
}

impl<'a, R: BufRead> Body<'a, R> {
    /// The body `f` frames on `r`; `None` for a body that runs until close.
    pub(crate) const fn new(r: &'a mut R, f: &Framing) -> Option<Self> {
        let (left, chunked) = match *f {
            Framing::Empty => (0, false),
            Framing::Length(n) => (n, false),
            Framing::Chunked => (0, true),
            Framing::UntilClose => return None,
        };
        let s = BodyState {
            left,
            chunked,
            begun: false,
            done: false,
        };
        Some(Self { r, s })
    }

    /// Goes on reading a body that stood at `s`.
    pub(crate) const fn resume(r: &'a mut R, s: BodyState) -> Self {
        Self { r, s }
    }

    /// Where the body stands now.
    pub(crate) const fn state(&self) -> BodyState {
        self.s
    }

    /// Reads one CRLF line of at most [`MAX_LINE`] bytes, without its ending.
    fn line(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        self.r.by_ref().take(MAX_LINE).read_until(b'\n', &mut buf)?;
        buf.strip_suffix(b"\r\n")
            .map(<[u8]>::to_vec)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "bad chunk framing"))
    }

    /// Reads the next chunk-size line, and the trailers after the last chunk.
    fn next_chunk(&mut self) -> io::Result<()> {
        let bad = || io::Error::new(ErrorKind::InvalidData, "bad chunk framing");
        if self.s.begun && !self.line()?.is_empty() {
            return Err(bad());
        }
        self.s.begun = true;
        let line = self.line()?;
        let text = std::str::from_utf8(&line).map_err(|_| bad())?;
        let size = text.split(';').next().unwrap_or_default();
        if size.is_empty() || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        self.s.left = u64::from_str_radix(size, 16).map_err(|_| bad())?;
        if self.s.left == 0 {
            for _ in 0..MAX_TRAILERS {
                if self.line()?.is_empty() {
                    self.s.done = true;
                    return Ok(());
                }
            }
            return Err(bad());
        }
        Ok(())
    }
}

impl<R: BufRead> Read for Body<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.s.left == 0 && self.s.chunked && !self.s.done {
            self.next_chunk()?;
        }
        if self.s.left == 0 || buf.is_empty() {
            return Ok(0);
        }
        let room = u64::try_from(buf.len())
            .unwrap_or(u64::MAX)
            .min(self.s.left);
        let n = self.r.by_ref().take(room).read(buf)?;
        if n == 0 {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        self.s.left -= n as u64;
        Ok(n)
    }
}

/// Sends `prefix`, then the rest of the body standing at `s` on `r`, framed as it came: a
/// length-framed body byte for byte, a chunked one in fresh chunks.
pub(crate) fn send_rest(
    r: &mut impl BufRead,
    w: &mut impl Write,
    prefix: &[u8],
    s: BodyState,
) -> io::Result<()> {
    let mut body = Body::resume(r, s);
    if !s.chunked {
        w.write_all(prefix)?;
        io::copy(&mut body, w)?;
        return Ok(());
    }
    let chunk = |data: &[u8], w: &mut dyn Write| -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        write!(w, "{:x}\r\n", data.len())?;
        w.write_all(data)?;
        w.write_all(b"\r\n")
    };
    chunk(prefix, w)?;
    let mut buf = vec![0_u8; CHUNK];
    loop {
        let n = body.read(&mut buf)?;
        if n == 0 {
            return w.write_all(b"0\r\n\r\n");
        }
        chunk(buf.get(..n).unwrap_or_default(), w)?;
    }
}
