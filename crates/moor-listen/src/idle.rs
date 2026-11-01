//! A reader that fails once its source has sent nothing for a while.

use std::io::{self, Cursor, ErrorKind, Read};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

/// Chunks the reader thread may hold ready before it waits.
const AHEAD: usize = 16;

/// Reads its source on a thread of its own, so a wait for bytes can give up after `idle`.
///
/// Once dropped, the thread ends at the source's next read, whatever it returns; the source's
/// own time limit bounds that.
pub(crate) struct Idle {
    /// Chunks as the thread read them; a read error ends them.
    chunks: Receiver<io::Result<Vec<u8>>>,
    /// The chunk being handed out.
    left: Cursor<Vec<u8>>,
    /// Longest wait for the next chunk.
    limit: Duration,
}

impl Idle {
    /// Starts reading `source`.
    pub(crate) fn new(mut source: impl Read + Send + 'static, idle: Duration) -> Self {
        let (tx, chunks) = sync_channel(AHEAD);
        thread::spawn(move || {
            let mut buf = vec![0; 8192];
            loop {
                let chunk = match source.read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => Ok(buf.get(..n).unwrap_or_default().to_vec()),
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => Err(e),
                };
                let last = chunk.is_err();
                if tx.send(chunk).is_err() || last {
                    return;
                }
            }
        });
        Self {
            chunks,
            left: Cursor::new(Vec::new()),
            limit: idle,
        }
    }
}

impl Read for Idle {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = self.left.read(out)?;
            if n > 0 || out.is_empty() {
                return Ok(n);
            }
            match self.chunks.recv_timeout(self.limit) {
                Ok(chunk) => self.left = Cursor::new(chunk?),
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
                Err(RecvTimeoutError::Timeout) => {
                    let why = format!("no bytes for {:?}", self.limit);
                    return Err(io::Error::new(ErrorKind::TimedOut, why));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use super::Idle;

    #[test]
    fn bytes_pass_through_and_silence_times_out() {
        let (mut near, far) = UnixStream::pair().unwrap();
        let mut idle = Idle::new(far, Duration::from_millis(200));
        near.write_all(b"ab").unwrap();
        let mut buf = [0; 8];
        assert_eq!(idle.read(&mut buf).unwrap(), 2);
        assert_eq!(buf.get(..2), Some(&b"ab"[..]));
        let err = idle.read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    }
}
