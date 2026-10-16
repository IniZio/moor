//! Stdin setup for spawned children.

use std::fs::File;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

use moor_core::seam::proc::Stdin;

/// Stdio for the child plus bytes to write to its pipe, if any.
pub(super) fn stdin_for(stdin: &Stdin) -> io::Result<(Stdio, Option<&[u8]>)> {
    match stdin {
        Stdin::Null => Ok((Stdio::null(), None)),
        Stdin::Bytes(b) => Ok((Stdio::piped(), Some(b.as_slice()))),
        Stdin::Secret(s) => Ok((Stdio::piped(), Some(s.expose()))),
        Stdin::File0600(p) => {
            let file = File::open(p)?;
            if file.metadata()?.permissions().mode() & 0o077 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "secret file must be mode 0600",
                ));
            }
            Ok((Stdio::from(file), None))
        }
    }
}
