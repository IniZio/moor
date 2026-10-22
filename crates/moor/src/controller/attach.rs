//! `moor controller attach`: bridges stdin/stdout to the local controller socket.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;

use moor_client::ATTACH_REFUSED_EXIT;

use crate::env::Env;

#[cfg(test)]
mod tests;

/// Bridge `input`/`output` to the controller socket until either side closes. Returns exit
/// code: 0 once the bridge ends, [`ATTACH_REFUSED_EXIT`] for a refused connect, else 1.
pub fn attach(
    env: &Env,
    socket: Option<&Path>,
    input: impl Read + Send + 'static,
    output: impl Write,
    stderr: impl Write,
) -> u8 {
    let mut stderr = stderr;
    let path = if let Some(p) = socket {
        env.expand_home(p)
    } else if let Some(p) = super::state_socket(env) {
        p
    } else {
        let _ = writeln!(
            stderr,
            "moor: no state directory: set state_dir or MOOR_STATE_DIR"
        );
        return 1;
    };
    let stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(
                stderr,
                "moor: controller at {} is unreachable: {e}",
                path.display()
            );
            return if e.kind() == std::io::ErrorKind::ConnectionRefused {
                ATTACH_REFUSED_EXIT
            } else {
                1
            };
        }
    };
    match bridge(stream, input, output) {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(stderr, "moor: controller connection failed: {e}");
            1
        }
    }
}

/// Copy stdin to the socket on a thread and the socket to `output`, flushing per chunk.
fn bridge(
    stream: UnixStream,
    mut input: impl Read + Send + 'static,
    mut output: impl Write,
) -> std::io::Result<()> {
    let mut up = stream.try_clone()?;
    // Detached: it may block on stdin forever after the controller closes.
    thread::spawn(move || {
        let _ = std::io::copy(&mut input, &mut up);
        let _ = up.shutdown(Shutdown::Write);
    });
    let mut down = stream;
    let mut buf = [0u8; 8192];
    loop {
        let n = match down.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if let Some(chunk) = buf.get(..n) {
            output.write_all(chunk)?;
        }
        output.flush()?;
    }
}
