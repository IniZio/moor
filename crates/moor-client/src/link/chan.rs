//! The frames a Box stream carries once a request has answered `streaming`.
//!
//! A frame is a five byte header, its kind and its payload's length as a big-endian `u32`, then
//! the payload. One codec serves captured and piped commands and terminals.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::boxes::Size;

/// The longest payload of one frame; a longer one is split by its writer and refused by its
/// reader, so a corrupt length never allocates much.
pub const MAX_PAYLOAD: usize = 1 << 20;

/// What a frame is, as its first byte says.
mod kind {
    /// Bytes for the command's stdin.
    pub(super) const IN: u8 = 0;
    /// The command's stdin ends.
    pub(super) const IN_END: u8 = 1;
    /// Bytes of the command's stdout; of its terminal when it has one.
    pub(super) const OUT: u8 = 2;
    /// Bytes of the command's stderr.
    pub(super) const ERR: u8 = 3;
    /// The terminal's new size.
    pub(super) const RESIZE: u8 = 4;
    /// How the command ended; the last frame.
    pub(super) const EXIT: u8 = 5;
}

/// How an exit frame's payload says the command ended.
mod ended {
    /// The command exited with a code, which follows.
    pub(super) const CODE: u8 = 0;
    /// A signal ended the command, whose number follows.
    pub(super) const SIGNAL: u8 = 1;
    /// The place the command ran in was down.
    pub(super) const DOWN: u8 = 2;
}

/// One frame of a command's stream; the controller sends `In`, `InEnd` and `Resize`, the machine
/// sends `Out`, `Err` and `Exit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame<'a> {
    /// Bytes for stdin.
    In(&'a [u8]),
    /// Stdin ends.
    InEnd,
    /// Bytes of stdout, or of the terminal.
    Out(&'a [u8]),
    /// Bytes of stderr.
    Err(&'a [u8]),
    /// The terminal's new size.
    Resize(Size),
    /// How the command ended.
    Exit(Exit),
}

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// It exited with this code.
    Code(i32),
    /// This signal ended it.
    Signal(i32),
    /// The place it ran in was down, so it did not run or was cut short by that.
    Down,
}

/// Writes `frame` and flushes.
///
/// # Errors
/// The stream is closed, or a payload is longer than [`MAX_PAYLOAD`].
pub async fn write_frame<W>(to: &mut W, frame: &Frame<'_>) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let (kind, payload) = match *frame {
        Frame::In(bytes) => (kind::IN, bytes.to_vec()),
        Frame::InEnd => (kind::IN_END, Vec::new()),
        Frame::Out(bytes) => (kind::OUT, bytes.to_vec()),
        Frame::Err(bytes) => (kind::ERR, bytes.to_vec()),
        Frame::Resize(Size { cols, rows }) => {
            let mut p = cols.to_be_bytes().to_vec();
            p.extend_from_slice(&rows.to_be_bytes());
            (kind::RESIZE, p)
        }
        Frame::Exit(Exit::Code(code)) => (kind::EXIT, with_number(ended::CODE, code)),
        Frame::Exit(Exit::Signal(signal)) => (kind::EXIT, with_number(ended::SIGNAL, signal)),
        Frame::Exit(Exit::Down) => (kind::EXIT, vec![ended::DOWN]),
    };
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|_| payload.len() <= MAX_PAYLOAD)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the frame is too long"))?;
    let mut bytes = Vec::with_capacity(5 + payload.len());
    bytes.push(kind);
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(&payload);
    to.write_all(&bytes).await?;
    to.flush().await
}

/// An exit payload: `tag`, then `n` big-endian.
fn with_number(tag: u8, n: i32) -> Vec<u8> {
    let mut p = vec![tag];
    p.extend_from_slice(&n.to_be_bytes());
    p
}

/// The next frame, or `None` when the stream ends between frames. `buf` holds the payload the
/// frame borrows.
///
/// # Errors
/// The stream ends inside a frame, a payload is longer than [`MAX_PAYLOAD`], or a frame is of a
/// kind or shape this release does not know.
pub async fn read_frame<'b, R>(from: &mut R, buf: &'b mut Vec<u8>) -> io::Result<Option<Frame<'b>>>
where
    R: AsyncRead + Unpin,
{
    let mut first = [0u8; 1];
    if from.read(&mut first).await? == 0 {
        return Ok(None);
    }
    let mut len = [0u8; 4];
    from.read_exact(&mut len).await?;
    let len = usize::try_from(u32::from_be_bytes(len)).unwrap_or(usize::MAX);
    if len > MAX_PAYLOAD {
        return Err(bad("the frame is too long"));
    }
    buf.clear();
    buf.resize(len, 0);
    from.read_exact(buf).await?;
    let payload = buf.as_slice();
    let [first] = first;
    let frame = match first {
        kind::IN => Frame::In(payload),
        kind::IN_END => Frame::InEnd,
        kind::OUT => Frame::Out(payload),
        kind::ERR => Frame::Err(payload),
        kind::RESIZE => match *payload {
            [c0, c1, r0, r1] => Frame::Resize(Size {
                cols: u16::from_be_bytes([c0, c1]),
                rows: u16::from_be_bytes([r0, r1]),
            }),
            _ => return Err(bad("a resize frame carries four bytes")),
        },
        kind::EXIT => Frame::Exit(
            match payload.split_first() {
                Some((&ended::CODE, n)) => number(n).map(Exit::Code),
                Some((&ended::SIGNAL, n)) => number(n).map(Exit::Signal),
                Some((&ended::DOWN, [])) => Some(Exit::Down),
                _ => None,
            }
            .ok_or_else(|| bad("not an exit frame"))?,
        ),
        other => return Err(bad(&format!("frame kind {other} is not known"))),
    };
    Ok(Some(frame))
}

/// The big-endian `i32` that is all of `bytes`.
fn number(bytes: &[u8]) -> Option<i32> {
    Some(i32::from_be_bytes(bytes.try_into().ok()?))
}

/// An invalid-data error saying `why`.
fn bad(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_owned())
}

#[cfg(test)]
mod tests;
